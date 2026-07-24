"""Emit formal metadata for an OpenAIRE parquet dataset, from the real parquet
schemas (schema only, no data scan):

  1. JSON Schema (draft 2020-12), one file per table, validating a single row:
       <root>/json-schema/<table>.schema.json
     Nullable columns get ["<type>","null"]; *_json / extra_json columns are
     strings tagged contentMediaType application/json.

  2. Croissant 1.0 (JSON-LD), one file for the whole dataset:
       <root>/croissant.jsonld
     Each parquet-<table> dir is a cr:FileSet; each table a cr:RecordSet whose
     cr:Fields map to the parquet columns with schema.org dataTypes.

Usage:
  python scripts/openaire/emit_metadata.py --root data/openaire/2026 --version 11.1.1
  python scripts/openaire/emit_metadata.py --root data/openaire       --version 3.0
"""

import argparse
import glob
import json
import os

import pyarrow as pa
import pyarrow.parquet as pq

LICENSE = "https://creativecommons.org/licenses/by/4.0/"
HOMEPAGE = "https://graph.openaire.eu/"
DOI = {"3.0": "https://doi.org/10.5281/zenodo.4707307",
       "11.1.1": "https://doi.org/10.5281/zenodo.20428976"}
DATE_PUBLISHED = {"3.0": "2021-04-27", "11.1.1": "2026-06-08"}

CROISSANT_CONTEXT = {
    "@language": "en",
    "@vocab": "https://schema.org/",
    "citeAs": "cr:citeAs",
    "column": "cr:column",
    "conformsTo": "dct:conformsTo",
    "cr": "http://mlcommons.org/croissant/",
    "data": {"@id": "cr:data", "@type": "@json"},
    "dataType": {"@id": "cr:dataType", "@type": "@vocab"},
    "dct": "http://purl.org/dc/terms/",
    "equivalentProperty": "cr:equivalentProperty",
    "examples": {"@id": "cr:examples", "@type": "@json"},
    "extract": "cr:extract",
    "field": "cr:field",
    "fileObject": "cr:fileObject",
    "fileProperty": "cr:fileProperty",
    "fileSet": "cr:fileSet",
    "format": "cr:format",
    "includes": "cr:includes",
    "isLiveDataset": "cr:isLiveDataset",
    "jsonPath": "cr:jsonPath",
    "key": "cr:key",
    "md5": "cr:md5",
    "parentField": "cr:parentField",
    "path": "cr:path",
    "rai": "http://mlcommons.org/croissant/RAI/",
    "recordSet": "cr:recordSet",
    "references": "cr:references",
    "regex": "cr:regex",
    "repeated": "cr:repeated",
    "replace": "cr:replace",
    "samplingRate": "cr:samplingRate",
    "sc": "https://schema.org/",
    "separator": "cr:separator",
    "source": "cr:source",
    "subField": "cr:subField",
    "transform": "cr:transform",
}


def json_schema_type(t):
    if pa.types.is_integer(t):
        return "integer"
    if pa.types.is_floating(t):
        return "number"
    if pa.types.is_boolean(t):
        return "boolean"
    return "string"


def croissant_type(t):
    if pa.types.is_integer(t):
        return "sc:Integer"
    if pa.types.is_floating(t):
        return "sc:Float"
    if pa.types.is_boolean(t):
        return "sc:Boolean"
    return "sc:Text"


def id_columns(field_names):
    for cand in ("id", "source_id", "person_id", "author1_id"):
        if cand in field_names:
            return [cand]
    return []


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--root", required=True)
    ap.add_argument("--version", required=True)
    args = ap.parse_args()

    base_id = f"https://w3id.org/rete/openaire/{args.version}"
    js_dir = os.path.join(args.root, "json-schema")
    os.makedirs(js_dir, exist_ok=True)

    distribution, record_sets = [], []
    n_tables = n_cols = 0

    for pdir in sorted(glob.glob(os.path.join(args.root, "parquet-*"))):
        files = sorted(glob.glob(os.path.join(pdir, "*.parquet")))
        if not files:
            continue
        table = os.path.basename(pdir).replace("parquet-", "")
        schema = pq.ParquetFile(files[0]).schema_arrow
        names = [f.name for f in schema]
        n_tables += 1
        n_cols += len(names)

        # --- 1. JSON Schema (draft 2020-12) ---
        props = {}
        for f in schema:
            jt = json_schema_type(f.type)
            spec = {"type": [jt, "null"] if f.nullable else jt}
            if f.name.endswith("_json"):
                spec["contentMediaType"] = "application/json"
                spec["description"] = "JSON-encoded nested value (string in parquet)."
            props[f.name] = spec
        js = {
            "$schema": "https://json-schema.org/draft/2020-12/schema",
            "$id": f"{base_id}/schema/{table}.schema.json",
            "title": f"OpenAIRE {args.version} — {table}",
            "description": f"One row of the OpenAIRE {table} parquet table.",
            "type": "object",
            "properties": props,
            "required": id_columns(names),
            "additionalProperties": False,
        }
        json.dump(js, open(os.path.join(js_dir, f"{table}.schema.json"), "w"), indent=1)

        # --- 2. Croissant FileSet + RecordSet ---
        fs_id = f"parquet-{table}"
        distribution.append({
            "@type": "cr:FileSet",
            "@id": fs_id,
            "name": fs_id,
            "description": f"Parquet files for the OpenAIRE {table} table.",
            "encodingFormat": "application/x-parquet",
            "includes": f"parquet-{table}/*.parquet",
        })
        record_sets.append({
            "@type": "cr:RecordSet",
            "@id": table,
            "name": table,
            "description": f"OpenAIRE {table} records.",
            "field": [
                {
                    "@type": "cr:Field",
                    "@id": f"{table}/{f.name}",
                    "name": f.name,
                    "dataType": croissant_type(f.type),
                    "source": {"fileSet": {"@id": fs_id}, "extract": {"column": f.name}},
                }
                for f in schema
            ],
        })

    croissant = {
        "@context": CROISSANT_CONTEXT,
        "@type": "Dataset",
        "conformsTo": "http://mlcommons.org/croissant/1.0",
        "name": f"openaire-graph-{args.version}",
        "description": (
            f"OpenAIRE Graph dataset v{args.version}, flattened to Parquet "
            f"({n_tables} tables). Typed scalar columns plus *_json columns holding "
            f"nested fields as JSON strings and an extra_json catch-all. See "
            f"openaire.ttl for the OWL vocabulary of the underlying data model."
        ),
        "url": HOMEPAGE,
        "sameAs": DOI.get(args.version),
        "license": LICENSE,
        "version": args.version,
        "datePublished": DATE_PUBLISHED.get(args.version),
        "citeAs": f"OpenAIRE Graph Dataset v{args.version}. {DOI.get(args.version, '')}",
        "distribution": distribution,
        "recordSet": record_sets,
    }
    out = os.path.join(args.root, "croissant.jsonld")
    json.dump(croissant, open(out, "w"), indent=1)
    print(f"[{args.version}] {n_tables} tables, {n_cols} columns")
    print(f"  json-schema/  -> {n_tables} *.schema.json")
    print(f"  {out}")


if __name__ == "__main__":
    main()
