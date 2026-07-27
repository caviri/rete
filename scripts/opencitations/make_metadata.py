"""Generate a JSON Schema and an MLCommons Croissant (1.0) JSON-LD description
for the OpenCitations Meta v13.1.0 Parquet table.

Writes:
  data/opencitations/meta-v13.1.0/schema.json       JSON Schema (draft 2020-12)
  data/opencitations/meta-v13.1.0/croissant.jsonld  Croissant 1.0 metadata
"""

import json
import os

OUT = r"D:\pro\rete\data\opencitations\meta-v13.1.0"
GLOB = "parquet/part-*.parquet"
N_ROWS = 135416506

# exact column order + duckdb types (from DESCRIBE)
COLS = [
    ("omid", "VARCHAR"), ("doi", "VARCHAR"), ("pmid", "VARCHAR"),
    ("openalex", "VARCHAR"), ("issn", "VARCHAR"), ("isbn", "VARCHAR"),
    ("pub_year", "INTEGER"), ("id", "VARCHAR"), ("title", "VARCHAR"),
    ("author", "VARCHAR"), ("venue", "VARCHAR"), ("volume", "VARCHAR"),
    ("issue", "VARCHAR"), ("page", "VARCHAR"), ("pub_date", "VARCHAR"),
    ("type", "VARCHAR"), ("publisher", "VARCHAR"), ("editor", "VARCHAR"),
]

DESC = {
    "omid": "OpenCitations Meta Identifier (e.g. br/0612345). Always present; the internal primary key.",
    "doi": "Digital Object Identifier, lowercased. Primary cross-dataset join key (DataCite / OpenAIRE / ORCID / DBLP).",
    "pmid": "PubMed identifier.",
    "openalex": "OpenAlex Work identifier (e.g. W2029384392).",
    "issn": "ISSN of the containing serial, when applicable.",
    "isbn": "ISBN, when applicable.",
    "pub_year": "Publication year (parsed from pub_date).",
    "id": "Raw space-separated list of all PIDs for the resource (omid: doi: pmid: openalex: issn: isbn: …), verbatim from the source.",
    "title": "Title of the bibliographic resource.",
    "author": "Authors, '; '-separated; each 'Family, Given [omid:ra/… orcid:…]' with embedded agent identifiers.",
    "venue": "Containing venue (journal, book, proceedings) as 'Name [omid:br/… issn:…]'.",
    "volume": "Volume designation.",
    "issue": "Issue designation.",
    "page": "Page range.",
    "pub_date": "Full publication date string (YYYY, YYYY-MM or YYYY-MM-DD).",
    "type": "Resource type (journal article, book chapter, dataset, …).",
    "publisher": "Publisher as 'Name [omid:ra/… crossref:…]'.",
    "editor": "Editors, '; '-separated; same shape as author.",
}

JSON_T = {"VARCHAR": "string", "INTEGER": "integer"}
CR_T = {"VARCHAR": "sc:Text", "INTEGER": "sc:Integer"}

LICENSE = "https://creativecommons.org/publicdomain/zero/1.0/"
HOMEPAGE = "https://opencitations.net/meta"
ZENODO = "https://doi.org/10.5281/zenodo.20965426"


def json_schema():
    props = {}
    for c, dt in COLS:
        typ = JSON_T[dt] if c == "omid" else [JSON_T[dt], "null"]
        props[c] = {"type": typ, "description": DESC[c]}
    return {
        "$schema": "https://json-schema.org/draft/2020-12/schema",
        "$id": "https://w3id.org/rete/opencitations-meta/schema.json",
        "title": "OpenCitations Meta v13.1.0 — Parquet row schema",
        "description": "One object = one bibliographic resource in the OpenCitations "
                       "Meta v13.1.0 dataset converted to Parquet. Source: "
                       f"{ZENODO} (CC0).",
        "type": "object",
        "properties": props,
        "required": ["omid"],
        "additionalProperties": False,
    }


CR_CONTEXT = {
    "@language": "en", "@vocab": "https://schema.org/",
    "citeAs": "cr:citeAs", "column": "cr:column", "conformsTo": "dct:conformsTo",
    "cr": "http://mlcommons.org/croissant/", "rai": "http://mlcommons.org/croissant/RAI/",
    "data": {"@id": "cr:data", "@type": "@json"},
    "dataType": {"@id": "cr:dataType", "@type": "@vocab"},
    "dct": "http://purl.org/dc/terms/",
    "examples": {"@id": "cr:examples", "@type": "@json"},
    "equivalentProperty": {"@id": "cr:equivalentProperty", "@type": "@vocab"},
    "samplingRate": "cr:samplingRate",
    "extract": "cr:extract", "field": "cr:field", "fileProperty": "cr:fileProperty",
    "fileObject": "cr:fileObject", "fileSet": "cr:fileSet", "format": "cr:format",
    "includes": "cr:includes", "isLiveDataset": "cr:isLiveDataset", "jsonPath": "cr:jsonPath",
    "key": "cr:key", "md5": "cr:md5", "parentField": "cr:parentField", "path": "cr:path",
    "recordSet": "cr:recordSet", "references": "cr:references", "regex": "cr:regex",
    "repeated": "cr:repeated", "replace": "cr:replace", "sc": "https://schema.org/",
    "separator": "cr:separator", "source": "cr:source", "subField": "cr:subField",
    "transform": "cr:transform",
}


def croissant():
    fields = [{
        "@type": "cr:Field", "@id": f"resource/{c}", "name": c,
        "description": DESC[c], "dataType": CR_T[dt],
        "source": {"fileSet": {"@id": "parquet_files"}, "extract": {"column": c}},
    } for c, dt in COLS]
    return {
        "@context": CR_CONTEXT,
        "@type": "sc:Dataset",
        "conformsTo": "http://mlcommons.org/croissant/1.0",
        "name": "opencitations-meta-v13.1.0",
        "description": "OpenCitations Meta v13.1.0 — bibliographic metadata for all "
                       "resources OpenCitations knows about (aggregated from Crossref, "
                       "DataCite, PubMed, JaLC, OpenAlex, OUTCITE and Matilda), "
                       "converted to Parquet. Each row carries the OMID plus DOI, PMID, "
                       "OpenAlex and ISSN/ISBN identifiers pulled out for joining — the "
                       "crosswalk hub of the scholarly graph.",
        "version": "13.1.0",
        "license": LICENSE,
        "url": HOMEPAGE,
        "sameAs": ZENODO,
        "citeAs": "OpenCitations (2026). OpenCitations Meta CSV dataset of all "
                  "bibliographic metadata, v13.1.0. Zenodo. " + ZENODO,
        "datePublished": "2026-06-27",
        "keywords": ["bibliographic metadata", "citations", "DOI", "OpenAlex",
                     "persistent identifiers", "scholarly communication", "crosswalk"],
        "creator": {"@type": "sc:Organization", "name": "OpenCitations",
                    "url": "https://opencitations.net"},
        "distribution": [{
            "@type": "cr:FileSet", "@id": "parquet_files", "name": "parquet_files",
            "description": "Parquet shards of the OpenCitations Meta table.",
            "encodingFormat": "application/x-parquet", "includes": GLOB,
        }],
        "recordSet": [{
            "@type": "cr:RecordSet", "@id": "resource", "name": "resource",
            "description": f"One bibliographic resource per row ({N_ROWS:,} rows).",
            "key": {"@id": "resource/omid"},
            "field": fields,
        }],
    }


def main():
    with open(os.path.join(OUT, "schema.json"), "w", encoding="utf-8") as f:
        json.dump(json_schema(), f, indent=2, ensure_ascii=False)
    with open(os.path.join(OUT, "croissant.jsonld"), "w", encoding="utf-8") as f:
        json.dump(croissant(), f, indent=2, ensure_ascii=False)
    print("wrote schema.json and croissant.jsonld:", len(COLS), "fields")


if __name__ == "__main__":
    main()
