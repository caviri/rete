"""Generate a JSON Schema and an MLCommons Croissant (1.0) JSON-LD description
for the DataCite Parquet datasets (metadata 2023/2024/2025 + PID Links
2023/May-2025).

Writes:
  data/datacite/schema.json       JSON Schema (draft 2020-12): 'metadata' + 'pid_link'
  data/datacite/croissant.jsonld  Croissant 1.0: 5 FileSets + 5 RecordSets
"""

import json
import os

OUT = r"D:\pro\rete\data\datacite"

METADATA_FULL = [  # 2024/2025 (51 cols)
    ("doi","VARCHAR"),("prefix","VARCHAR"),("state","VARCHAR"),("source","VARCHAR"),
    ("is_active","BOOLEAN"),("client_id","VARCHAR"),("created","VARCHAR"),
    ("registered","VARCHAR"),("updated","VARCHAR"),("published","VARCHAR"),
    ("publication_year","INTEGER"),("language","VARCHAR"),("version","VARCHAR"),
    ("metadata_version","INTEGER"),("schema_version","VARCHAR"),("url","VARCHAR"),
    ("publisher","VARCHAR"),("resource_type_general","VARCHAR"),("resource_type","VARCHAR"),
    ("reason","VARCHAR"),("title","VARCHAR"),
    ("citation_count","INTEGER"),("reference_count","INTEGER"),("view_count","INTEGER"),
    ("download_count","INTEGER"),("version_count","INTEGER"),("version_of_count","INTEGER"),
    ("part_count","INTEGER"),("part_of_count","INTEGER"),
    ("types_json","VARCHAR"),("container_json","VARCHAR"),("creators_json","VARCHAR"),
    ("titles_json","VARCHAR"),("subjects_json","VARCHAR"),("contributors_json","VARCHAR"),
    ("dates_json","VARCHAR"),("related_identifiers_json","VARCHAR"),
    ("related_items_json","VARCHAR"),("descriptions_json","VARCHAR"),
    ("geo_locations_json","VARCHAR"),("funding_references_json","VARCHAR"),
    ("rights_list_json","VARCHAR"),("identifiers_json","VARCHAR"),
    ("alternate_identifiers_json","VARCHAR"),("sizes_json","VARCHAR"),
    ("formats_json","VARCHAR"),("content_url_json","VARCHAR"),
    ("citations_over_time_json","VARCHAR"),("views_over_time_json","VARCHAR"),
    ("downloads_over_time_json","VARCHAR"),("extra_json","VARCHAR"),
]
METRIC_COLS = {"citation_count","reference_count","view_count","download_count",
               "version_count","version_of_count","part_count","part_of_count",
               "citations_over_time_json","views_over_time_json","downloads_over_time_json"}
METADATA_2023 = [(c, t) for c, t in METADATA_FULL if c not in METRIC_COLS]  # 40 cols

PIDLINK = [
    ("subj_id","VARCHAR"),("obj_id","VARCHAR"),("relation_type","VARCHAR"),
    ("source_id","VARCHAR"),("citation_type","VARCHAR"),("subj_type","VARCHAR"),
    ("obj_type","VARCHAR"),("subj_published","VARCHAR"),("obj_published","VARCHAR"),
    ("subj_year","INTEGER"),("obj_year","INTEGER"),("occurred_at","VARCHAR"),
    ("created_at","VARCHAR"),("updated_at","VARCHAR"),("uuid","VARCHAR"),
    ("subj_extra_json","VARCHAR"),("obj_extra_json","VARCHAR"),("extra_json","VARCHAR"),
]

DESC = {
    "doi":"DOI of the record. Primary cross-dataset join key (OpenCitations / OpenAIRE / ORCID / DBLP); case per DataCite, lowercase on join.",
    "prefix":"DOI prefix (the registrant's namespace, e.g. 10.5281).",
    "state":"Record state; the public file contains only 'findable'.",
    "source":"How the record was registered (e.g. mds, api).",
    "is_active":"Whether the DOI is active.",
    "client_id":"Repository (DataCite client) that registered the DOI. Present in 2024+ files.",
    "created":"Record creation timestamp (ISO 8601).",
    "registered":"DOI registration timestamp (ISO 8601).",
    "updated":"Last update timestamp (ISO 8601).",
    "published":"Publication date string as supplied.",
    "publication_year":"Publication year.",
    "language":"Primary language of the resource.",
    "version":"Version label of the resource.",
    "metadata_version":"DataCite metadata version counter.",
    "schema_version":"DataCite metadata schema version (e.g. http://datacite.org/schema/kernel-4).",
    "url":"Landing-page URL the DOI resolves to.",
    "publisher":"Publisher name (2024+ publisher objects flattened to name; original in extra_json.publisher_obj).",
    "resource_type_general":"DataCite general resource type (Dataset, Text, Software, Image, PhysicalObject, Collection, …).",
    "resource_type":"Free-text resource type as supplied.",
    "reason":"Reason a DOI is not findable (usually null here).",
    "title":"First title (convenience; full list in titles_json).",
    "citation_count":"Citation count (2024+ files only).",
    "reference_count":"Reference count (2024+ only).",
    "view_count":"View count (2024+ only).",
    "download_count":"Download count (2024+ only).",
    "version_count":"Number of versions (2024+ only).",
    "version_of_count":"Number of works this is a version of (2024+ only).",
    "part_count":"Number of parts (2024+ only).",
    "part_of_count":"Number of works this is part of (2024+ only).",
    "types_json":"JSON object of the DataCite `types` block (schemaOrg, resourceTypeGeneral, citeproc, bibtex, ris).",
    "container_json":"JSON of the container (e.g. hosting repository/series).",
    "creators_json":"JSON array of creators: name, givenName, familyName, nameIdentifiers (ORCID), affiliation.",
    "titles_json":"JSON array of all titles.",
    "subjects_json":"JSON array of subjects/keywords.",
    "contributors_json":"JSON array of contributors with roles.",
    "dates_json":"JSON array of typed dates.",
    "related_identifiers_json":"JSON array of related identifiers: relationType, relatedIdentifier, type. The citation/derivation network.",
    "related_items_json":"JSON array of related items (structured citations).",
    "descriptions_json":"JSON array of descriptions/abstracts.",
    "geo_locations_json":"JSON array of geo locations.",
    "funding_references_json":"JSON array of funding references (funder, award number).",
    "rights_list_json":"JSON array of rights/licenses.",
    "identifiers_json":"JSON array of alternate identifiers of this resource.",
    "alternate_identifiers_json":"JSON array of legacy alternate identifiers.",
    "sizes_json":"JSON array of sizes.","formats_json":"JSON array of formats.",
    "content_url_json":"JSON array/string of content URLs.",
    "citations_over_time_json":"JSON time series of citations (2024+ only).",
    "views_over_time_json":"JSON time series of views (2024+ only).",
    "downloads_over_time_json":"JSON time series of downloads (2024+ only).",
    "extra_json":"JSON catch-all of any attribute key not promoted to its own column.",
    # pid_link
    "subj_id":"Subject PID (bare DOI when a doi.org URL, else full URL). Join key to metadata.doi.",
    "obj_id":"Object PID (bare DOI or URL). Join key to metadata.doi.",
    "relation_type":"Relation, e.g. references, cites, is-supplement-to, is-identical-to, is-derived-from, is-authored-by.",
    "source_id":"Provenance of the relation (crossref, datacite-related, datacite-crossref, …).",
    "citation_type":"schema.org type pair, e.g. ScholarlyArticle-Dataset.",
    "subj_type":"schema.org @type of the subject endpoint.",
    "obj_type":"schema.org @type of the object endpoint.",
    "subj_published":"Publication date of the subject endpoint.",
    "obj_published":"Publication date of the object endpoint.",
    "subj_year":"Publication year of the subject.","obj_year":"Publication year of the object.",
    "occurred_at":"When the relation occurred (0000-01-01… = unknown).",
    "created_at":"Event creation timestamp.","updated_at":"Event update timestamp.",
    "uuid":"Event UUID.",
    "subj_extra_json":"JSON of subject endpoint fields beyond id/@type/date_published.",
    "obj_extra_json":"JSON of object endpoint fields beyond id/@type/date_published.",
}

COUNTS = {"metadata_2023":52863283,"metadata_2024":72019577,"metadata_2025":108468906,
          "pidlinks_2023":167844248,"pidlinks_may2025":592958301}
GLOBS = {"metadata_2023":"parquet-2023/part-*.parquet","metadata_2024":"parquet-2024/part-*.parquet",
         "metadata_2025":"parquet-2025/part-*.parquet","pidlinks_2023":"parquet-links-2023/part-*.parquet",
         "pidlinks_may2025":"parquet-links-may2025/part-*.parquet"}
RS_COLS = {"metadata_2023":METADATA_2023,"metadata_2024":METADATA_FULL,"metadata_2025":METADATA_FULL,
           "pidlinks_2023":PIDLINK,"pidlinks_may2025":PIDLINK}

JSON_T = {"VARCHAR":"string","INTEGER":"integer","BOOLEAN":"boolean"}
CR_T = {"VARCHAR":"sc:Text","INTEGER":"sc:Integer","BOOLEAN":"sc:Boolean"}
LICENSE = "https://creativecommons.org/publicdomain/zero/1.0/"


def props(cols, required_null_ok):
    out = {}
    for c, dt in cols:
        typ = JSON_T[dt] if c in required_null_ok else [JSON_T[dt], "null"]
        p = {"type": typ, "description": DESC[c]}
        if c.endswith("_json"):
            p["contentMediaType"] = "application/json"
        out[c] = p
    return out


def json_schema():
    return {
        "$schema":"https://json-schema.org/draft/2020-12/schema",
        "$id":"https://w3id.org/rete/datacite/schema.json",
        "title":"DataCite Public Data Files — Parquet row schemas",
        "description":"Row schemas for the DataCite Public Data Files converted to "
                      "Parquet. 'metadata' = one DOI record (metric columns present in "
                      "2024+ files only); 'pid_link' = one PID Graph relationship event. "
                      "Source: https://datafiles.datacite.org/ (CC0).",
        "$defs":{
            "metadata":{"type":"object","title":"DataCite metadata record",
                "description":"One row per DOI. Metric columns (citation_count, …, "
                              "*_over_time_json) appear only in the 2024 and 2025 files.",
                "properties":props(METADATA_FULL,{"doi"}),"required":["doi"],
                "additionalProperties":False},
            "pid_link":{"type":"object","title":"DataCite PID Graph relationship",
                "description":"One row per relationship event between two PIDs.",
                "properties":props(PIDLINK,{"subj_id","obj_id","relation_type"}),
                "required":["subj_id","obj_id","relation_type"],
                "additionalProperties":False},
        },
    }


CR_CONTEXT = {
    "@language":"en","@vocab":"https://schema.org/","citeAs":"cr:citeAs","column":"cr:column",
    "conformsTo":"dct:conformsTo","cr":"http://mlcommons.org/croissant/","rai":"http://mlcommons.org/croissant/RAI/",
    "data":{"@id":"cr:data","@type":"@json"},"dataType":{"@id":"cr:dataType","@type":"@vocab"},
    "dct":"http://purl.org/dc/terms/","examples":{"@id":"cr:examples","@type":"@json"},
    "equivalentProperty":{"@id":"cr:equivalentProperty","@type":"@vocab"},"samplingRate":"cr:samplingRate",
    "extract":"cr:extract","field":"cr:field","fileProperty":"cr:fileProperty","fileObject":"cr:fileObject",
    "fileSet":"cr:fileSet","format":"cr:format","includes":"cr:includes","isLiveDataset":"cr:isLiveDataset",
    "jsonPath":"cr:jsonPath","key":"cr:key","md5":"cr:md5","parentField":"cr:parentField","path":"cr:path",
    "recordSet":"cr:recordSet","references":"cr:references","regex":"cr:regex","repeated":"cr:repeated",
    "replace":"cr:replace","sc":"https://schema.org/","separator":"cr:separator","source":"cr:source",
    "subField":"cr:subField","transform":"cr:transform",
}


def croissant():
    distribution, record_sets = [], []
    for rs, cols in RS_COLS.items():
        fs = rs + "_files"
        distribution.append({"@type":"cr:FileSet","@id":fs,"name":fs,
            "description":f"Parquet shards for {rs}.","encodingFormat":"application/x-parquet",
            "includes":GLOBS[rs]})
        key = "doi" if rs.startswith("metadata") else "subj_id"
        fields = [{"@type":"cr:Field","@id":f"{rs}/{c}","name":c,"description":DESC[c],
                   "dataType":CR_T[dt],"source":{"fileSet":{"@id":fs},"extract":{"column":c}}}
                  for c, dt in cols]
        record_sets.append({"@type":"cr:RecordSet","@id":rs,"name":rs,
            "description":f"{'DOI metadata records' if rs.startswith('metadata') else 'PID Graph relationship events'} — {COUNTS[rs]:,} rows.",
            "field":fields})
    return {
        "@context":CR_CONTEXT,"@type":"sc:Dataset","conformsTo":"http://mlcommons.org/croissant/1.0",
        "name":"datacite-public-data-files","description":"The DataCite Public Data Files "
            "converted to Parquet: DOI metadata for 2023, 2024 and 2025, plus the PID Graph "
            "'PID Links' relationship events for 2023 and May 2025. Metadata rows carry the DOI, "
            "resource type, creators (with ORCIDs), publisher and the related-identifier network; "
            "PID Links are the citation/derivation edge tables.",
        "version":"1.0.0","datePublished":"2026-01-06",
        "license":LICENSE,"url":"https://datafiles.datacite.org/",
        "citeAs":"DataCite (2023–2025). DataCite Public Data File. https://datafiles.datacite.org/",
        "keywords":["DOI","research data","citations","persistent identifiers","metadata","PID graph"],
        "creator":{"@type":"sc:Organization","name":"DataCite","url":"https://datacite.org"},
        "distribution":distribution,"recordSet":record_sets,
    }


def main():
    with open(os.path.join(OUT,"schema.json"),"w",encoding="utf-8") as f:
        json.dump(json_schema(),f,indent=2,ensure_ascii=False)
    with open(os.path.join(OUT,"croissant.jsonld"),"w",encoding="utf-8") as f:
        json.dump(croissant(),f,indent=2,ensure_ascii=False)
    print("schema.json: 2 $defs (metadata 51 / pid_link 18); croissant:",
          len(RS_COLS),"FileSets +",len(RS_COLS),"RecordSets")


if __name__ == "__main__":
    main()
