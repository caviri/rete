"""Generate a JSON Schema and an MLCommons Croissant (1.0) JSON-LD description
for the ORCID Public Data File 2025 Parquet tables (summaries + activities).

Writes:
  data/orcid/schema.json      JSON Schema (draft 2020-12), one $def per table
  data/orcid/croissant.jsonld Croissant 1.0 dataset metadata (FileSets +
                              relational RecordSets, orcid as person key)
"""

import json
import os

OUT = r"D:\pro\rete\data\orcid"

# (group/table) -> [(column, duckdb_type)] — exact, from DESCRIBE of the parquet
SCHEMA = {
    "summaries/person": [("orcid","VARCHAR"),("given_names","VARCHAR"),("family_name","VARCHAR"),("credit_name","VARCHAR"),("biography","VARCHAR"),("locale","VARCHAR"),("creation_method","VARCHAR"),("submission_date","VARCHAR"),("last_modified_date","VARCHAR"),("claimed","BOOLEAN"),("verified_email","BOOLEAN"),("country","VARCHAR"),("n_other_names","INTEGER"),("n_keywords","INTEGER"),("n_external_ids","INTEGER"),("n_works","INTEGER"),("n_employments","INTEGER"),("n_educations","INTEGER"),("n_fundings","INTEGER"),("n_distinctions","INTEGER"),("n_invited_positions","INTEGER"),("n_memberships","INTEGER"),("n_qualifications","INTEGER"),("n_services","INTEGER"),("n_peer_reviews","INTEGER"),("n_research_resources","INTEGER"),("other_names_json","VARCHAR"),("keywords_json","VARCHAR"),("external_ids_json","VARCHAR"),("researcher_urls_json","VARCHAR")],
    "summaries/work": [("orcid","VARCHAR"),("put_code","VARCHAR"),("title","VARCHAR"),("subtitle","VARCHAR"),("journal_title","VARCHAR"),("type","VARCHAR"),("pub_year","INTEGER"),("pub_month","INTEGER"),("pub_day","INTEGER"),("doi","VARCHAR"),("url","VARCHAR"),("source_name","VARCHAR"),("external_ids_json","VARCHAR")],
    "summaries/affiliation": [("orcid","VARCHAR"),("aff_type","VARCHAR"),("put_code","VARCHAR"),("department","VARCHAR"),("role_title","VARCHAR"),("start_year","INTEGER"),("start_month","INTEGER"),("start_day","INTEGER"),("end_year","INTEGER"),("end_month","INTEGER"),("end_day","INTEGER"),("org_name","VARCHAR"),("org_city","VARCHAR"),("org_region","VARCHAR"),("org_country","VARCHAR"),("org_id","VARCHAR"),("org_id_source","VARCHAR"),("source_name","VARCHAR")],
    "summaries/funding": [("orcid","VARCHAR"),("put_code","VARCHAR"),("title","VARCHAR"),("type","VARCHAR"),("start_year","INTEGER"),("end_year","INTEGER"),("org_name","VARCHAR"),("org_country","VARCHAR"),("org_id","VARCHAR"),("org_id_source","VARCHAR"),("source_name","VARCHAR"),("external_ids_json","VARCHAR")],
    "activities/work": [("orcid","VARCHAR"),("put_code","VARCHAR"),("title","VARCHAR"),("subtitle","VARCHAR"),("translated_title","VARCHAR"),("journal_title","VARCHAR"),("type","VARCHAR"),("pub_year","INTEGER"),("pub_month","INTEGER"),("pub_day","INTEGER"),("doi","VARCHAR"),("url","VARCHAR"),("language_code","VARCHAR"),("country","VARCHAR"),("short_description","VARCHAR"),("source_name","VARCHAR"),("n_contributors","INTEGER"),("external_ids_json","VARCHAR"),("contributors_json","VARCHAR")],
    "activities/affiliation": [("orcid","VARCHAR"),("aff_type","VARCHAR"),("put_code","VARCHAR"),("department","VARCHAR"),("role_title","VARCHAR"),("start_year","INTEGER"),("start_month","INTEGER"),("start_day","INTEGER"),("end_year","INTEGER"),("end_month","INTEGER"),("end_day","INTEGER"),("org_name","VARCHAR"),("org_city","VARCHAR"),("org_region","VARCHAR"),("org_country","VARCHAR"),("org_id","VARCHAR"),("org_id_source","VARCHAR"),("url","VARCHAR"),("source_name","VARCHAR")],
    "activities/funding": [("orcid","VARCHAR"),("put_code","VARCHAR"),("title","VARCHAR"),("type","VARCHAR"),("org_defined_type","VARCHAR"),("start_year","INTEGER"),("end_year","INTEGER"),("amount","VARCHAR"),("currency","VARCHAR"),("org_name","VARCHAR"),("org_country","VARCHAR"),("org_id","VARCHAR"),("org_id_source","VARCHAR"),("source_name","VARCHAR"),("external_ids_json","VARCHAR"),("contributors_json","VARCHAR")],
    "activities/peer_review": [("orcid","VARCHAR"),("put_code","VARCHAR"),("reviewer_role","VARCHAR"),("review_type","VARCHAR"),("review_group_id","VARCHAR"),("completion_year","INTEGER"),("org_name","VARCHAR"),("org_country","VARCHAR"),("org_id","VARCHAR"),("org_id_source","VARCHAR"),("source_name","VARCHAR"),("review_ids_json","VARCHAR")],
    "activities/research_resource": [("orcid","VARCHAR"),("put_code","VARCHAR"),("title","VARCHAR"),("start_year","INTEGER"),("end_year","INTEGER"),("hosts_json","VARCHAR"),("external_ids_json","VARCHAR"),("source_name","VARCHAR")],
}

COUNTS = {
    "summaries/person": 25048058, "summaries/work": 149782968,
    "summaries/affiliation": 25063901, "summaries/funding": 1838095,
    "activities/work": 149933459, "activities/affiliation": 24102382,
    "activities/funding": 1839676, "activities/peer_review": 20408908,
    "activities/research_resource": 5983,
}

TABLE_DESC = {
    "summaries/person": "One row per ORCID researcher: name, country, locale, history flags, per-activity counts, and person-level keyword/other-name/external-id/URL lists (JSON).",
    "summaries/work": "One row per work-summary from the record summary: title, type, publication year, journal and the first DOI (join key).",
    "summaries/affiliation": "One row per employment/education/distinction/invited-position/membership/qualification/service summary, with organization + disambiguated org id (ROR/GRID/RINGGOLD/FUNDREF).",
    "summaries/funding": "One row per funding (grant) summary.",
    "activities/work": "One row per full work record (from the activities files): adds contributors (co-authorship), abstract, language and country to the work-summary fields.",
    "activities/affiliation": "One row per full affiliation record across the seven affiliation types.",
    "activities/funding": "One row per full funding record: adds amount, currency, organization-defined type and contributors.",
    "activities/peer_review": "One row per peer-review activity: reviewer role, review type/group and convening organization.",
    "activities/research_resource": "One row per research-resource activity.",
}

# shared per-column descriptions
COL = {
    "orcid": "ORCID iD of the researcher this row belongs to (path form, e.g. 0000-0002-7869-831X).",
    "put_code": "ORCID put-code: stable identifier of this activity within the record.",
    "doi": "First DOI in the activity's external identifiers, lowercased. Join key to DataCite / OpenAIRE / DBLP / OpenCitations.",
    "title": "Title of the work / funding / resource.",
    "subtitle": "Subtitle, when present.",
    "translated_title": "Translated title, when present.",
    "journal_title": "Container / journal title.",
    "type": "Activity type (e.g. journal-article, book-chapter; for funding: grant, contract).",
    "pub_year": "Publication year.", "pub_month": "Publication month.", "pub_day": "Publication day.",
    "url": "Canonical URL of the activity.",
    "language_code": "ISO language code of the work.",
    "country": "Country code associated with the activity.",
    "short_description": "Abstract / short description of the work.",
    "source_name": "Name of the source that asserted the activity (e.g. Crossref, Scopus - Elsevier, or the researcher).",
    "n_contributors": "Number of contributors (co-authors) recorded for the work.",
    "contributors_json": "JSON array of contributors: [{name, orcid, role, seq}]. The co-authorship edge list.",
    "external_ids_json": "JSON array of external identifiers: [{type, value, url, relationship}].",
    "aff_type": "Affiliation type: employment | education | distinction | invited-position | membership | qualification | service.",
    "department": "Department name.", "role_title": "Role or degree title.",
    "start_year": "Start year.", "start_month": "Start month.", "start_day": "Start day.",
    "end_year": "End year.", "end_month": "End month.", "end_day": "End day.",
    "org_name": "Organization name.", "org_city": "Organization city.",
    "org_region": "Organization region.", "org_country": "Organization country code.",
    "org_id": "Disambiguated organization identifier (e.g. a ROR/GRID/RINGGOLD/FUNDREF id). Join key to organization graphs.",
    "org_id_source": "Scheme of org_id: ROR | GRID | RINGGOLD | FUNDREF | LEI.",
    "org_defined_type": "Organization-defined funding sub-type.",
    "amount": "Funding amount (as stored).", "currency": "ISO currency code of the amount.",
    "reviewer_role": "Reviewer role (e.g. reviewer).", "review_type": "Review type (e.g. review, evaluation).",
    "review_group_id": "Review group id (e.g. issn:0167-6369).",
    "completion_year": "Year the review was completed.",
    "review_ids_json": "JSON array of review external identifiers.",
    "hosts_json": "JSON array of host organization names for the research resource.",
    "given_names": "Given name(s).", "family_name": "Family name.", "credit_name": "Published/credit name.",
    "biography": "Free-text biography.", "locale": "Account locale.",
    "creation_method": "How the record was created (e.g. Direct, Member-referred).",
    "submission_date": "Record submission timestamp (ISO 8601).",
    "last_modified_date": "Last modification timestamp (ISO 8601).",
    "claimed": "Whether the record has been claimed by the individual.",
    "verified_email": "Whether the record has a verified email.",
    "other_names_json": "JSON array of alternative names.",
    "keywords_json": "JSON array of self-declared keywords.",
    "researcher_urls_json": "JSON array of researcher URLs: [{name, url}].",
}
for k in ("n_other_names","n_keywords","n_external_ids","n_works","n_employments",
          "n_educations","n_fundings","n_distinctions","n_invited_positions",
          "n_memberships","n_qualifications","n_services","n_peer_reviews",
          "n_research_resources"):
    COL[k] = "Count of " + k[2:].replace("_", " ") + " on the record."

JSON_TYPE = {"VARCHAR": "string", "INTEGER": "integer", "BOOLEAN": "boolean"}
CR_TYPE = {"VARCHAR": "sc:Text", "INTEGER": "sc:Integer", "BOOLEAN": "sc:Boolean"}

LICENSE = "https://creativecommons.org/publicdomain/zero/1.0/"
HOMEPAGE = "https://orcid.figshare.com/articles/dataset/ORCID_Public_Data_File_2025/30375589"
DOI = "https://doi.org/10.23640/07243.30375589.v1"


def tid(t):
    return t.replace("/", "_")


def rel_glob(t):
    grp, name = t.split("/")
    return f"parquet-{grp}/{name}/part-*.parquet"


def build_json_schema():
    defs = {}
    for t, cols in SCHEMA.items():
        props = {}
        for c, dt in cols:
            jt = JSON_TYPE[dt]
            # every column is nullable except the orcid key
            typ = jt if c == "orcid" else [jt, "null"]
            p = {"type": typ, "description": COL.get(c, c)}
            if c.endswith("_json"):
                p["contentMediaType"] = "application/json"
            props[c] = p
        defs[tid(t)] = {
            "type": "object",
            "title": t,
            "description": f"{TABLE_DESC[t]} ({COUNTS[t]:,} rows.)",
            "properties": props,
            "required": ["orcid"],
            "additionalProperties": False,
        }
    return {
        "$schema": "https://json-schema.org/draft/2020-12/schema",
        "$id": "https://w3id.org/rete/orcid-2025/schema.json",
        "title": "ORCID Public Data File 2025 — Parquet table schemas",
        "description": "Row schemas for the ORCID Public Data File 2025 converted "
                       "to Parquet (summaries + activities). One $def per table; "
                       "each object is one row. Source: "
                       "https://orcid.figshare.com/articles/dataset/ORCID_Public_Data_File_2025/30375589 (CC0).",
        "$defs": defs,
    }


CR_CONTEXT = {
    "@language": "en",
    "@vocab": "https://schema.org/",
    "citeAs": "cr:citeAs", "column": "cr:column", "conformsTo": "dct:conformsTo",
    "cr": "http://mlcommons.org/croissant/", "rai": "http://mlcommons.org/croissant/RAI/",
    "data": {"@id": "cr:data", "@type": "@json"},
    "dataType": {"@id": "cr:dataType", "@type": "@vocab"}, "dct": "http://purl.org/dc/terms/",
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


def build_croissant():
    distribution = []
    record_sets = []
    for t, cols in SCHEMA.items():
        fs_id = tid(t) + "_files"
        distribution.append({
            "@type": "cr:FileSet", "@id": fs_id, "name": fs_id,
            "description": f"Parquet shards for the {t} table.",
            "encodingFormat": "application/x-parquet",
            "includes": rel_glob(t),
        })
        fields = []
        for c, dt in cols:
            field = {
                "@type": "cr:Field", "@id": f"{tid(t)}/{c}", "name": c,
                "description": COL.get(c, c), "dataType": CR_TYPE[dt],
                "source": {"fileSet": {"@id": fs_id}, "extract": {"column": c}},
            }
            # relational links: orcid on non-person tables references person key
            if c == "orcid" and t != "summaries/person":
                field["references"] = {"field": {"@id": "summaries_person/orcid"}}
            fields.append(field)
        rs = {
            "@type": "cr:RecordSet", "@id": tid(t), "name": tid(t),
            "description": f"{TABLE_DESC[t]} ({COUNTS[t]:,} rows.)",
            "field": fields,
        }
        if t == "summaries/person":
            rs["key"] = {"@id": "summaries_person/orcid"}
        record_sets.append(rs)

    return {
        "@context": CR_CONTEXT,
        "@type": "sc:Dataset",
        "conformsTo": "http://mlcommons.org/croissant/1.0",
        "name": "orcid-public-data-file-2025",
        "description": "The ORCID Public Data File 2025 (snapshot of all public "
                       "ORCID records as of 2025-10-01) converted to Parquet: nine "
                       "relational tables across the record summaries and the full "
                       "activities. Includes works (with DOIs), affiliations (with "
                       "ROR/GRID org ids), fundings, peer reviews and the "
                       "co-authorship contributor lists.",
        "version": "2025",
        "license": LICENSE,
        "url": HOMEPAGE,
        "citeAs": "Pinto et al.; ORCID (2025). ORCID Public Data File 2025. "
                  "figshare. " + DOI,
        "sameAs": DOI,
        "datePublished": "2025-10-20",
        "keywords": ["ORCID", "researchers", "scholarly metadata", "co-authorship",
                     "affiliations", "bibliometrics", "persistent identifiers"],
        "creator": {"@type": "sc:Organization", "name": "ORCID, Inc.",
                    "url": "https://orcid.org"},
        "distribution": distribution,
        "recordSet": record_sets,
    }


def main():
    js = build_json_schema()
    with open(os.path.join(OUT, "schema.json"), "w", encoding="utf-8") as f:
        json.dump(js, f, indent=2, ensure_ascii=False)
    cr = build_croissant()
    with open(os.path.join(OUT, "croissant.jsonld"), "w", encoding="utf-8") as f:
        json.dump(cr, f, indent=2, ensure_ascii=False)
    print("wrote schema.json:", sum(len(v["properties"]) for v in js["$defs"].values()),
          "fields across", len(js["$defs"]), "tables")
    print("wrote croissant.jsonld:", len(cr["distribution"]), "FileSets,",
          len(cr["recordSet"]), "RecordSets")


if __name__ == "__main__":
    main()
