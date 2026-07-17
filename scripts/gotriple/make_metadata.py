"""Generate a JSON Schema and an MLCommons Croissant (1.0) JSON-LD description
for the GoTriple Parquet dataset. Row count is read from the Parquet footers.

Writes:
  data/go-triple/schema.json       JSON Schema (draft 2020-12): 'document'
  data/go-triple/croissant.jsonld  Croissant 1.0: 1 FileSet + 1 RecordSet
"""

import glob
import json
import os

import pyarrow.parquet as pq

OUT = r"D:\pro\rete\data\go-triple"
GLOB = "parquet/*.parquet"
LICENSE = "https://creativecommons.org/publicdomain/zero/1.0/"

DOC = [
    ("id", "VARCHAR"), ("type", "VARCHAR"), ("doi", "VARCHAR"), ("title", "VARCHAR"),
    ("discipline", "VARCHAR"), ("primary_topic", "VARCHAR"),
    ("date_published", "VARCHAR"), ("datestamp", "VARCHAR"), ("language", "VARCHAR"),
    ("provider", "VARCHAR"), ("publisher", "VARCHAR"), ("url", "VARCHAR"),
    ("is_cluster", "BOOLEAN"), ("is_duplicate", "BOOLEAN"),
    ("cluster_children_count", "INTEGER"), ("n_authors", "INTEGER"),
    ("doi_json", "VARCHAR"), ("headline_json", "VARCHAR"), ("abstract_json", "VARCHAR"),
    ("author_json", "VARCHAR"), ("contributor_json", "VARCHAR"),
    ("keywords_json", "VARCHAR"), ("knows_about_json", "VARCHAR"),
    ("topic_json", "VARCHAR"), ("provider_json", "VARCHAR"),
    ("publisher_json", "VARCHAR"), ("producer_json", "VARCHAR"),
    ("in_language_json", "VARCHAR"), ("license_json", "VARCHAR"),
    ("conditions_of_access_json", "VARCHAR"), ("identifier_json", "VARCHAR"),
    ("url_json", "VARCHAR"), ("main_entity_of_page_json", "VARCHAR"),
    ("spatial_coverage_json", "VARCHAR"), ("temporal_coverage_json", "VARCHAR"),
    ("mentions_json", "VARCHAR"), ("additional_type_json", "VARCHAR"),
    ("original_languages_json", "VARCHAR"), ("original_document_types_json", "VARCHAR"),
    ("original_license_json", "VARCHAR"), ("original_conditions_of_access_json", "VARCHAR"),
    ("cluster_id_json", "VARCHAR"), ("discarded_keywords_json", "VARCHAR"),
    ("discarded_authors_json", "VARCHAR"), ("extra_json", "VARCHAR"),
]

DESC = {
    "id": "Native GoTriple document id (e.g. oai:doaj.org/article:…). Primary key.",
    "type": "schema.org @type (Document).",
    "doi": "First non-empty DOI. Cross-dataset join key (DataCite/Zenodo/OpenAIRE/OpenCitations); lowercase on join. ~69% of records carry one.",
    "title": "First headline text (convenience; full multilingual list in headline_json).",
    "discipline": "GoTriple discipline the record is grouped under (the source filename label): hist, socio, droit, psy, …",
    "primary_topic": "Highest-confidence topic discipline id (full scored list in topic_json).",
    "date_published": "Publication date string as supplied.",
    "datestamp": "OAI datestamp of the record.",
    "language": "First language code (full list in in_language_json).",
    "provider": "First source aggregator/repository (e.g. doaj, DNB, serval.unil.ch).",
    "publisher": "First publisher name.",
    "url": "First full-text / landing URL.",
    "is_cluster": "Whether the record is a duplicate-cluster representative.",
    "is_duplicate": "Whether GoTriple flagged the record as a duplicate.",
    "cluster_children_count": "Number of records in the cluster (null if not a cluster).",
    "n_authors": "Number of authors (convenience; full list in author_json).",
    "doi_json": "JSON array of all DOIs.",
    "headline_json": "JSON array of titles as CommonTranslatedLabel (lang + text + translation flags).",
    "abstract_json": "JSON array of abstracts as CommonTranslatedLabel (multilingual, with machine translations).",
    "author_json": "JSON array of authors {agg, fullname, id}.",
    "contributor_json": "JSON array of contributors.",
    "keywords_json": "JSON array of keywords as CommonTranslatedLabel.",
    "knows_about_json": "JSON array of linked SSH-LCSH subject authorities {uri, labels:[{lang,text}]} (semantics.gr).",
    "topic_json": "JSON array of confidence-scored topics {id (discipline), confidence}.",
    "provider_json": "JSON array of source providers.",
    "publisher_json": "JSON array of publishers.",
    "producer_json": "JSON array of producers.",
    "in_language_json": "JSON array of language codes.",
    "license_json": "JSON array of licenses.",
    "conditions_of_access_json": "JSON array of access conditions (acr_open-access / acr_closed-access / …).",
    "identifier_json": "JSON array of identifiers (ISSN, DOI, …).",
    "url_json": "JSON array of URLs.",
    "main_entity_of_page_json": "JSON array of main-entity-of-page URLs.",
    "spatial_coverage_json": "JSON array of spatial coverage codes.",
    "temporal_coverage_json": "JSON array of temporal coverage values.",
    "mentions_json": "JSON array of mentioned entities.",
    "additional_type_json": "JSON array of additional GoTriple type codes (e.g. typ_article).",
    "original_languages_json": "JSON array of the source record's original languages.",
    "original_document_types_json": "JSON array of the source record's original document types.",
    "original_license_json": "JSON array of the source record's original license.",
    "original_conditions_of_access_json": "JSON array of the source record's original access conditions.",
    "cluster_id_json": "JSON array of cluster ids.",
    "discarded_keywords_json": "JSON array of keywords dropped during processing.",
    "discarded_authors_json": "JSON array of authors dropped during processing.",
    "extra_json": "JSON catch-all of any field not promoted to its own column.",
}

JSON_T = {"VARCHAR": "string", "INTEGER": "integer", "BOOLEAN": "boolean"}
CR_T = {"VARCHAR": "sc:Text", "INTEGER": "sc:Integer", "BOOLEAN": "sc:Boolean"}


def count_rows():
    return sum(pq.ParquetFile(p).metadata.num_rows
              for p in glob.glob(os.path.join(OUT, GLOB)))


def props():
    out = {}
    for c, dt in DOC:
        typ = JSON_T[dt] if c == "id" else [JSON_T[dt], "null"]
        p = {"type": typ, "description": DESC[c]}
        if c.endswith("_json"):
            p["contentMediaType"] = "application/json"
        out[c] = p
    return out


def json_schema():
    return {
        "$schema": "https://json-schema.org/draft/2020-12/schema",
        "$id": "https://w3id.org/rete/gotriple/schema.json",
        "title": "GoTriple metadata dataset — Parquet row schema",
        "description": "Row schema for the GoTriple metadata dataset (Zenodo 18185971, CC0) "
                       "converted to Parquet: one row per SSH publication. Scalars are typed "
                       "columns; every nested GoTriple field is kept whole as a JSON string.",
        "$defs": {
            "document": {"type": "object", "title": "GoTriple document",
                "description": "One SSH publication indexed by GoTriple (with a full-text link).",
                "properties": props(), "required": ["id"], "additionalProperties": False},
        },
    }


CR_CONTEXT = {
    "@language": "en", "@vocab": "https://schema.org/", "citeAs": "cr:citeAs", "column": "cr:column",
    "conformsTo": "dct:conformsTo", "cr": "http://mlcommons.org/croissant/", "rai": "http://mlcommons.org/croissant/RAI/",
    "data": {"@id": "cr:data", "@type": "@json"}, "dataType": {"@id": "cr:dataType", "@type": "@vocab"},
    "dct": "http://purl.org/dc/terms/", "examples": {"@id": "cr:examples", "@type": "@json"},
    "extract": "cr:extract", "field": "cr:field", "fileProperty": "cr:fileProperty", "fileObject": "cr:fileObject",
    "fileSet": "cr:fileSet", "format": "cr:format", "includes": "cr:includes", "isLiveDataset": "cr:isLiveDataset",
    "jsonPath": "cr:jsonPath", "key": "cr:key", "md5": "cr:md5", "parentField": "cr:parentField", "path": "cr:path",
    "recordSet": "cr:recordSet", "references": "cr:references", "regex": "cr:regex", "repeated": "cr:repeated",
    "replace": "cr:replace", "sc": "https://schema.org/", "separator": "cr:separator", "source": "cr:source",
    "subField": "cr:subField", "transform": "cr:transform",
}


def croissant(n):
    fields = [{"@type": "cr:Field", "@id": f"document/{c}", "name": c, "description": DESC[c],
               "dataType": CR_T[dt], "source": {"fileSet": {"@id": "document_files"}, "extract": {"column": c}}}
              for c, dt in DOC]
    return {
        "@context": CR_CONTEXT, "@type": "sc:Dataset", "conformsTo": "http://mlcommons.org/croissant/1.0",
        "name": "gotriple-metadata",
        "description": "The GoTriple metadata dataset converted to Parquet: metadata for 6,074,813 "
            "Social-Sciences & Humanities publications with a full-text link, harvested from the "
            "EU-funded GoTriple discovery platform (gotriple.eu / TRIPLE / OPERAS). Each record "
            "carries a DOI, multilingual title/abstract, its GoTriple discipline, confidence-scored "
            "topics, linked SSH-LCSH subject authorities, authors, source provider and full-text URL. "
            "DOI-keyed, so it federates with the DataCite/Zenodo/OpenAIRE/OpenCitations scholarly graph.",
        "version": "1.0.0", "datePublished": "2026-07-17",
        "license": LICENSE, "url": "https://zenodo.org/records/18185971",
        "citeAs": "Singh, H., Bertozzi, A., De Santis, L., Romanello, M. (2026). GoTriple metadata dataset. "
                  "Zenodo. https://doi.org/10.5281/zenodo.18185971",
        "keywords": ["SSH", "social sciences", "humanities", "DOI", "scholarly", "open access",
                     "GoTriple", "OPERAS", "multilingual", "metadata"],
        "creator": {"@type": "sc:Organization", "name": "GoTriple / OPERAS", "url": "https://www.gotriple.eu"},
        "distribution": [{"@type": "cr:FileSet", "@id": "document_files", "name": "document_files",
            "description": "Parquet shards (one per discipline).",
            "encodingFormat": "application/x-parquet", "includes": GLOB}],
        "recordSet": [{"@type": "cr:RecordSet", "@id": "document", "name": "document",
            "description": f"SSH publication metadata records — {n:,} rows.", "field": fields}],
    }


def main():
    n = count_rows()
    with open(os.path.join(OUT, "schema.json"), "w", encoding="utf-8") as f:
        json.dump(json_schema(), f, indent=2, ensure_ascii=False)
    with open(os.path.join(OUT, "croissant.jsonld"), "w", encoding="utf-8") as f:
        json.dump(croissant(n), f, indent=2, ensure_ascii=False)
    print(f"rows: {n:,}; schema.json (1 $def, {len(DOC)} cols) + croissant (1 FileSet + 1 RecordSet)")


if __name__ == "__main__":
    main()
