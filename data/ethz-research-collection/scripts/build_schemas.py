#!/usr/bin/env python3
"""schema.json (JSON Schema 2020-12) + croissant.jsonld (Croissant 1.0) for the
ETH Research Collection per-class entity Parquet tables.

Reads companions/ethz-research-collection-tables/*.parquet (one file = one table),
types each column from the real Arrow schema, and points every Croissant
cr:FileObject at its public R2 URL. Reuses the canonical Croissant @context.
Run in Docker (pyarrow, jsonschema, mlcroissant)."""
import glob
import json
import os
import pyarrow as pa
import pyarrow.parquet as pq

TABLES = "data/ethz-research-collection/companions/ethz-research-collection-tables"
OUT = "data/ethz-research-collection/schemas"
R2 = "https://data.graphplaza.com/ethz-research-collection/ethz-research-collection-tables"
NAME = "ethz-research-collection"
VERSION = "1.0.0"
LICENSE = "https://creativecommons.org/publicdomain/zero/1.0/"
URL = "https://www.research-collection.ethz.ch/"

CTX = {
    "@language": "en", "@vocab": "https://schema.org/",
    "citeAs": "cr:citeAs", "column": "cr:column", "conformsTo": "dct:conformsTo",
    "cr": "http://mlcommons.org/croissant/",
    "data": {"@id": "cr:data", "@type": "@json"},
    "dataType": {"@id": "cr:dataType", "@type": "@vocab"},
    "dct": "http://purl.org/dc/terms/",
    "equivalentProperty": "cr:equivalentProperty",
    "examples": {"@id": "cr:examples", "@type": "@json"},
    "extract": "cr:extract", "field": "cr:field", "fileObject": "cr:fileObject",
    "fileProperty": "cr:fileProperty", "fileSet": "cr:fileSet", "format": "cr:format",
    "includes": "cr:includes", "isLiveDataset": "cr:isLiveDataset",
    "jsonPath": "cr:jsonPath", "key": "cr:key", "md5": "cr:md5",
    "parentField": "cr:parentField", "path": "cr:path",
    "rai": "http://mlcommons.org/croissant/RAI/",
    "recordSet": "cr:recordSet", "references": "cr:references", "regex": "cr:regex",
    "repeated": "cr:repeated", "replace": "cr:replace",
    "samplingRate": "cr:samplingRate", "sc": "https://schema.org/",
    "separator": "cr:separator", "source": "cr:source", "subField": "cr:subField",
    "transform": "cr:transform",
}
TABLE_DESC = {
    "works": "Publications & research data (one row per item), with title, type, issue date, DOI, handle, availability, language, publisher, licence, abstract, and list columns for authors (IRIs) and subjects.",
    "persons": "Authors, editors and supervisors (one row per person). ORCID-identified people are minted at https://orcid.org/{orcid}.",
    "files": "The ORIGINAL-bundle file manifest — one row per deposited bitstream: name, MIME, size (bytes), MD5 checksum, download URL, bundle. Metadata only; no bytes.",
    "journals": "Serials/journals (one row per journal), ISSN-identified where available.",
    "grants": "Grants funding the works (name, identifier, programme).",
    "units": "ETH Leitzahl organisational units (department / institute / chair-group tree).",
    "triples": "The entire graph as a flat, lossless (subject, predicate, object) table with decoded value/datatype/lang columns.",
}


def js_type(t):
    if pa.types.is_integer(t): return "integer"
    if pa.types.is_floating(t) or pa.types.is_decimal(t): return "number"
    if pa.types.is_boolean(t): return "boolean"
    if pa.types.is_list(t) or pa.types.is_large_list(t): return "array"
    return "string"

def js_spec(t):
    if js_type(t) == "array":
        return {"type": "array", "items": js_spec(t.value_type)}
    return {"type": js_type(t)}

def cr_type(t):
    if pa.types.is_integer(t): return "sc:Integer"
    if pa.types.is_floating(t) or pa.types.is_decimal(t): return "sc:Float"
    if pa.types.is_boolean(t): return "sc:Boolean"
    if pa.types.is_list(t) or pa.types.is_large_list(t): return cr_type(t.value_type)
    return "sc:Text"


def main():
    os.makedirs(OUT, exist_ok=True)
    defs, dist, rsets = {}, [], []
    total = 0
    for f in sorted(glob.glob(f"{TABLES}/*.parquet")):
        tname = os.path.basename(f)[:-8]
        pf = pq.ParquetFile(f)
        schema = pf.schema_arrow
        rows = pf.metadata.num_rows
        total += rows
        props = {}
        for fld in schema:
            spec = js_spec(fld.type)
            if fld.nullable:
                spec["type"] = [spec["type"], "null"] if isinstance(spec["type"], str) else spec["type"]
            props[fld.name] = spec
        defs[tname] = {"type": "object", "title": tname,
                       "description": f"{TABLE_DESC.get(tname,'')} ({rows:,} rows)",
                       "properties": props, "additionalProperties": False}
        fo = f"{tname}.parquet"
        dist.append({"@type": "cr:FileObject", "@id": fo, "name": fo,
                     "description": TABLE_DESC.get(tname, ""),
                     "contentUrl": f"{R2}/{fo}",
                     "encodingFormat": "application/x-parquet",
                     "sha256": "unknown"})
        rsets.append({"@type": "cr:RecordSet", "@id": tname, "name": tname,
                      "description": f"{TABLE_DESC.get(tname,'')} ({rows:,} rows)",
                      "field": [dict({"@type": "cr:Field", "@id": f"{tname}/{fld.name}",
                                 "name": fld.name, "dataType": cr_type(fld.type),
                                 "source": {"fileObject": {"@id": fo}, "extract": {"column": fld.name}}},
                                 **({"repeated": True}
                                    if pa.types.is_list(fld.type) or pa.types.is_large_list(fld.type)
                                    else {}))
                                for fld in schema]})

    schema_doc = {"$schema": "https://json-schema.org/draft/2020-12/schema",
                  "$id": f"https://w3id.org/rete/{NAME}/schema.json",
                  "title": f"{NAME} — Parquet table schemas",
                  "description": "Row schemas for the ETH Research Collection per-class entity Parquet companion tables.",
                  "$defs": defs}
    croissant = {"@context": CTX, "@type": "Dataset",
                 "conformsTo": "http://mlcommons.org/croissant/1.0",
                 "name": NAME,
                 "description": "ETH Zürich Research Collection (DSpace 7.6) scholarly graph — per-class entity tables (works, persons, files, journals, grants, units) plus the flat triples table. Companions to the ethz-research-collection.rete graph; aligned to the rete scholar hub via canonical DOI/ORCID/ISSN IRIs.",
                 "url": URL, "license": LICENSE, "version": VERSION,
                 "citeAs": "ETH Research Collection (research-collection.ethz.ch), harvested via OAI-PMH.",
                 "datePublished": "2026-07-23",
                 "distribution": dist, "recordSet": rsets}

    json.dump(schema_doc, open(f"{OUT}/schema.json", "w", encoding="utf-8"), indent=1, ensure_ascii=False)
    json.dump(croissant, open(f"{OUT}/croissant.jsonld", "w", encoding="utf-8"), indent=1, ensure_ascii=False)
    print(f"wrote schema.json ({len(defs)} tables, {total:,} rows) + croissant.jsonld")

    from jsonschema import Draft202012Validator
    Draft202012Validator.check_schema(schema_doc)
    print("  schema.json: VALID (draft 2020-12)")
    try:
        import mlcroissant as mlc
        ds = mlc.Dataset(jsonld=f"{OUT}/croissant.jsonld")
        print(f"  croissant.jsonld: VALID ({len(ds.metadata.record_sets)} recordSets)")
    except ImportError:
        print("  (mlcroissant not installed — croissant not validated)")


if __name__ == "__main__":
    main()
