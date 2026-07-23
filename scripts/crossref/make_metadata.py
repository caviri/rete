"""Generate a JSON Schema and an MLCommons Croissant (1.0) JSON-LD description
for the Crossref Parquet datasets (works + refs).

Writes:
  data/crossref/schema.json       JSON Schema (draft 2020-12): 'work' + 'reference'
  data/crossref/croissant.jsonld  Croissant 1.0: 2 FileSets + 2 RecordSets
"""

import json
import os

OUT = r"D:\pro\rete\data\crossref"

WORKS = [
    ("doi", "VARCHAR"), ("prefix", "VARCHAR"), ("member", "VARCHAR"),
    ("type", "VARCHAR"), ("title", "VARCHAR"), ("subtitle", "VARCHAR"),
    ("original_title", "VARCHAR"), ("container_title", "VARCHAR"),
    ("short_container_title", "VARCHAR"), ("publisher", "VARCHAR"),
    ("publisher_location", "VARCHAR"), ("volume", "VARCHAR"), ("issue", "VARCHAR"),
    ("page", "VARCHAR"), ("article_number", "VARCHAR"), ("language", "VARCHAR"),
    ("issn", "VARCHAR"), ("issn_json", "VARCHAR"), ("isbn", "VARCHAR"),
    ("isbn_json", "VARCHAR"), ("issued", "VARCHAR"), ("issued_year", "INTEGER"),
    ("published", "VARCHAR"), ("published_print", "VARCHAR"),
    ("published_online", "VARCHAR"), ("accepted", "VARCHAR"), ("created", "VARCHAR"),
    ("deposited", "VARCHAR"), ("indexed", "VARCHAR"),
    ("is_referenced_by_count", "INTEGER"), ("references_count", "INTEGER"),
    ("abstract", "VARCHAR"), ("resource_url", "VARCHAR"), ("update_policy", "VARCHAR"),
    ("author_json", "VARCHAR"), ("editor_json", "VARCHAR"), ("translator_json", "VARCHAR"),
    ("chair_json", "VARCHAR"), ("license_json", "VARCHAR"), ("link_json", "VARCHAR"),
    ("funder_json", "VARCHAR"), ("assertion_json", "VARCHAR"), ("relation_json", "VARCHAR"),
    ("update_to_json", "VARCHAR"), ("updated_by_json", "VARCHAR"),
    ("alternative_id_json", "VARCHAR"), ("archive_json", "VARCHAR"),
    ("event_json", "VARCHAR"), ("institution_json", "VARCHAR"),
    ("journal_issue_json", "VARCHAR"), ("content_domain_json", "VARCHAR"),
    ("clinical_trial_number_json", "VARCHAR"), ("aliases_json", "VARCHAR"),
    ("free_to_read_json", "VARCHAR"), ("review_json", "VARCHAR"),
    ("standards_body_json", "VARCHAR"), ("subject_json", "VARCHAR"),
    ("extra_json", "VARCHAR"),
]

REFS = [
    ("doi", "VARCHAR"), ("ref_index", "INTEGER"), ("key", "VARCHAR"),
    ("ref_doi", "VARCHAR"), ("doi_asserted_by", "VARCHAR"), ("year", "INTEGER"),
    ("unstructured", "VARCHAR"), ("rest_json", "VARCHAR"),
]

DESC = {
    # ---- works
    "doi": "DOI of the work, lowercased. Primary cross-dataset join key (DataCite / OpenAIRE / ORCID / DBLP / OpenCitations). Instance IRI: https://doi.org/{doi}.",
    "prefix": "DOI prefix (the registrant's namespace, e.g. 10.1038).",
    "member": "Crossref member id (the depositing organisation/publisher).",
    "type": "Crossref work type: journal-article, book-chapter, proceedings-article, dataset, posted-content, component, …",
    "title": "First title (convenience; full array in extra_json.title_rest when >1).",
    "subtitle": "First subtitle.",
    "original_title": "First original-language title.",
    "container_title": "Journal / book / proceedings title (first entry).",
    "short_container_title": "Abbreviated container title (first entry).",
    "publisher": "Publisher name.",
    "publisher_location": "Publisher location.",
    "volume": "Volume.", "issue": "Issue.", "page": "Page range.",
    "article_number": "Article number.",
    "language": "Language code (ISO 639).",
    "issn": "First ISSN (convenience; full list with types in issn_json).",
    "issn_json": "JSON array of {value,type} ISSNs (issn-type).",
    "isbn": "First ISBN (convenience; full list in isbn_json).",
    "isbn_json": "JSON array of {value,type} ISBNs (isbn-type).",
    "issued": "Earliest known publication date, YYYY[-MM[-DD]] (partial dates kept partial).",
    "issued_year": "Year of `issued`, normalised to a real 1000–2030 value or NULL.",
    "published": "Published date (union of print/online), YYYY[-MM[-DD]].",
    "published_print": "Print publication date.",
    "published_online": "Online publication date.",
    "accepted": "Acceptance date.",
    "created": "Crossref record creation timestamp (ISO 8601 date-time).",
    "deposited": "Last metadata deposit timestamp (ISO 8601 date-time).",
    "indexed": "Last Crossref indexing timestamp (ISO 8601 date-time).",
    "is_referenced_by_count": "Number of Crossref works citing this one (inbound citations).",
    "references_count": "Number of references this work deposited (outbound; count of `refs` rows).",
    "abstract": "Abstract as a JATS XML string (present for ~22% of works).",
    "resource_url": "Primary resource URL (publisher landing page); other resource keys in extra_json.resource.",
    "update_policy": "DOI of the update policy (Crossmark).",
    "author_json": "JSON array of authors: given, family, sequence, affiliation, ORCID.",
    "editor_json": "JSON array of editors.",
    "translator_json": "JSON array of translators.",
    "chair_json": "JSON array of chairs.",
    "license_json": "JSON array of licenses: URL, content-version, start date, delay.",
    "link_json": "JSON array of full-text links: URL, content-type, intended-application.",
    "funder_json": "JSON array of funders: name, DOI (Crossref Funder ID), award list. IRI form: https://doi.org/10.13039/{funder-id}.",
    "assertion_json": "JSON array of publisher assertions (Crossmark).",
    "relation_json": "JSON object of typed relations to other works/entities (has-preprint, is-version-of, has-review, …).",
    "update_to_json": "JSON array of works this record updates (corrections/retractions).",
    "updated_by_json": "JSON array of works that update this record.",
    "alternative_id_json": "JSON array of publisher alternative ids.",
    "archive_json": "JSON array of archiving programmes (CLOCKSS, Portico, …).",
    "event_json": "JSON object describing the event (for conference proceedings).",
    "institution_json": "JSON array of institutions.",
    "journal_issue_json": "JSON object of journal-issue details (issue, published dates).",
    "content_domain_json": "JSON object of Crossmark content-domain / crossmark-restriction.",
    "clinical_trial_number_json": "JSON array of clinical trial numbers + registry.",
    "aliases_json": "JSON array of alias DOIs.",
    "free_to_read_json": "JSON object of free-to-read window.",
    "review_json": "JSON object of peer-review metadata (type, stage, competing-interest).",
    "standards_body_json": "JSON object of the standards body (for standards).",
    "subject_json": "JSON array of subject/category labels (legacy; largely retired by Crossref).",
    "extra_json": "JSON catch-all of any field not promoted to its own column, plus *_raw for values that overflowed a typed column and *_rest for dropped array tails.",
    # ---- refs
    "ref_index": "Position of the reference within the citing work (0-based).",
    "key": "Crossref per-reference key.",
    "ref_doi": "Cited DOI, lowercased — NULL when the reference is unstructured / not DOI-matched. Join key to work.doi.",
    "doi_asserted_by": "Who asserted the cited DOI: 'crossref' (matched) or 'publisher' (deposited).",
    "year": "Cited work's year, normalised to a real 1000–2030 value or NULL; unrecoverable originals kept in rest_json.year_raw.",
    "unstructured": "Raw citation text when no structured metadata was deposited.",
    "rest_json": "JSON of all other reference fields (author, volume, first-page, journal-title, article-title, ISSN, issue, …) plus year_raw.",
}

COUNTS = {"works": 179536204, "refs": 2742943747}
GLOBS = {"works": "parquet-2026/works/part-*.parquet",
         "refs": "parquet-2026/refs/part-*.parquet"}
RS_COLS = {"works": WORKS, "refs": REFS}
REQUIRED = {"works": ["doi"], "refs": ["doi", "ref_index"]}
RS_LABEL = {"works": "Crossref work (one DOI-registered record)",
            "refs": "Crossref reference (one citation edge)"}

JSON_T = {"VARCHAR": "string", "INTEGER": "integer", "BOOLEAN": "boolean"}
CR_T = {"VARCHAR": "sc:Text", "INTEGER": "sc:Integer", "BOOLEAN": "sc:Boolean"}
LICENSE = "https://creativecommons.org/licenses/by/4.0/"


def props(cols, required):
    out = {}
    for c, dt in cols:
        typ = JSON_T[dt] if c in required else [JSON_T[dt], "null"]
        p = {"type": typ, "description": DESC[c]}
        if c.endswith("_json"):
            p["contentMediaType"] = "application/json"
        out[c] = p
    return out


def json_schema():
    return {
        "$schema": "https://json-schema.org/draft/2020-12/schema",
        "$id": "https://w3id.org/rete/crossref/schema.json",
        "title": "Crossref Public Data File — Parquet row schemas",
        "description": "Row schemas for the Crossref Public Data File (March 2026) "
                       "converted to Parquet. 'work' = one DOI-registered record; "
                       "'reference' = one entry of a work's reference list (the citation "
                       "edge list). Source: https://www.crossref.org/learning/public-data-file/ "
                       "(CC-BY 4.0).",
        "$defs": {
            "work": {
                "type": "object", "title": "Crossref work",
                "description": "One row per DOI. Scalars are typed columns; nested Crossref "
                               "fields are kept whole as JSON strings; the reference list is "
                               "split out into the 'reference' table.",
                "properties": props(WORKS, {"doi"}),
                "required": REQUIRED["works"], "additionalProperties": False,
            },
            "reference": {
                "type": "object", "title": "Crossref reference (citation edge)",
                "description": "One row per reference entry across all works. doi (citing) "
                               "-> ref_doi (cited) is a citation when ref_doi is present.",
                "properties": props(REFS, {"doi", "ref_index"}),
                "required": REQUIRED["refs"], "additionalProperties": False,
            },
        },
    }


CR_CONTEXT = {
    "@language": "en", "@vocab": "https://schema.org/", "citeAs": "cr:citeAs", "column": "cr:column",
    "conformsTo": "dct:conformsTo", "cr": "http://mlcommons.org/croissant/", "rai": "http://mlcommons.org/croissant/RAI/",
    "data": {"@id": "cr:data", "@type": "@json"}, "dataType": {"@id": "cr:dataType", "@type": "@vocab"},
    "dct": "http://purl.org/dc/terms/", "examples": {"@id": "cr:examples", "@type": "@json"},
    "equivalentProperty": {"@id": "cr:equivalentProperty", "@type": "@vocab"}, "samplingRate": "cr:samplingRate",
    "extract": "cr:extract", "field": "cr:field", "fileProperty": "cr:fileProperty", "fileObject": "cr:fileObject",
    "fileSet": "cr:fileSet", "format": "cr:format", "includes": "cr:includes", "isLiveDataset": "cr:isLiveDataset",
    "jsonPath": "cr:jsonPath", "key": "cr:key", "md5": "cr:md5", "parentField": "cr:parentField", "path": "cr:path",
    "recordSet": "cr:recordSet", "references": "cr:references", "regex": "cr:regex", "repeated": "cr:repeated",
    "replace": "cr:replace", "sc": "https://schema.org/", "separator": "cr:separator", "source": "cr:source",
    "subField": "cr:subField", "transform": "cr:transform",
}


def croissant():
    distribution, record_sets = [], []
    for rs, cols in RS_COLS.items():
        fs = rs + "_files"
        distribution.append({
            "@type": "cr:FileSet", "@id": fs, "name": fs,
            "description": f"Parquet shards for the Crossref {rs} table (499 parts).",
            "encodingFormat": "application/x-parquet", "includes": GLOBS[rs],
        })
        fields = [{
            "@type": "cr:Field", "@id": f"{rs}/{c}", "name": c, "description": DESC[c],
            "dataType": CR_T[dt], "source": {"fileSet": {"@id": fs}, "extract": {"column": c}},
        } for c, dt in cols]
        record_sets.append({
            "@type": "cr:RecordSet", "@id": rs, "name": rs,
            "description": f"{RS_LABEL[rs]} — {COUNTS[rs]:,} rows.",
            "field": fields,
        })
    # link refs.doi and refs.ref_doi to works.doi (the citation graph join)
    for rs_field in ("refs/doi", "refs/ref_doi"):
        for rec in record_sets:
            for fld in rec["field"]:
                if fld["@id"] == rs_field:
                    fld["references"] = {"field": {"@id": "works/doi"}}
    return {
        "@context": CR_CONTEXT, "@type": "sc:Dataset",
        "conformsTo": "http://mlcommons.org/croissant/1.0",
        "name": "crossref-public-data-file-2026",
        "description": "The Crossref Public Data File (March 2026) converted to Parquet. "
            "'works' is one row per DOI-registered record (~179.5M): title, type, container, "
            "dates, ISSN/ISBN, publisher, funders and authors (with ORCIDs). 'refs' is the "
            "citation edge list (~2.74B rows, ~2.0B DOI-matched): work.doi cites ref_doi. "
            "Joins the DataCite / OpenAIRE / ORCID / DBLP / OpenCitations Parquet datasets on DOI.",
        "version": "1.0.0", "datePublished": "2026-07-17",
        "license": LICENSE, "url": "https://www.crossref.org/learning/public-data-file/",
        "citeAs": "Crossref (2026). Crossref Public Data File. https://doi.org/10.13003/nggf-vt1j",
        "keywords": ["DOI", "scholarly metadata", "citations", "references",
                     "bibliographic", "persistent identifiers", "open citations"],
        "creator": {"@type": "sc:Organization", "name": "Crossref", "url": "https://www.crossref.org"},
        "distribution": distribution, "recordSet": record_sets,
    }


def main():
    os.makedirs(OUT, exist_ok=True)
    with open(os.path.join(OUT, "schema.json"), "w", encoding="utf-8") as f:
        json.dump(json_schema(), f, indent=2, ensure_ascii=False)
    with open(os.path.join(OUT, "croissant.jsonld"), "w", encoding="utf-8") as f:
        json.dump(croissant(), f, indent=2, ensure_ascii=False)
    print(f"schema.json: 2 $defs (work {len(WORKS)} / reference {len(REFS)}); "
          f"croissant: 2 FileSets + 2 RecordSets")


if __name__ == "__main__":
    main()
