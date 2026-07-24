"""JSON Schema + MLCommons Croissant (1.0) for the ROR Parquet table."""

import json
import os

OUT = r"D:\pro\rete\data\ror"
N_ROWS = 111068

COLS = [
    ("id","string"),("ror_id","string"),("name","string"),("status","string"),
    ("established","integer"),("primary_type","string"),("country_code","string"),
    ("country_name","string"),("location_name","string"),("lat","number"),
    ("lng","number"),("geonames_id","integer"),("website","string"),
    ("wikipedia","string"),("fundref","string"),("grid","string"),("isni","string"),
    ("wikidata","string"),("n_relationships","integer"),("n_names","integer"),
    ("created_date","string"),("last_modified_date","string"),("types_json","string"),
    ("names_json","string"),("locations_json","string"),("links_json","string"),
    ("external_ids_json","string"),("relationships_json","string"),("domains_json","string"),
]
DESC = {
    "id":"ROR ID as a URL (https://ror.org/…). The organization's IRI.",
    "ror_id":"Bare ROR ID (e.g. 04ttjf776). The cross-dataset join key: matches the org_id/ROR values in ORCID affiliations, EPFL org ids, etc.",
    "name":"Display name (the ror_display / label name).",
    "status":"Registry status: active, inactive or withdrawn.",
    "established":"Year the organization was established.",
    "primary_type":"First ROR type: education, funder, healthcare, company, archive, nonprofit, government, facility, other.",
    "country_code":"ISO country code of the primary location.",
    "country_name":"Country name of the primary location.",
    "location_name":"City/place name of the primary location.",
    "lat":"Latitude of the primary location.","lng":"Longitude of the primary location.",
    "geonames_id":"GeoNames id of the primary location.",
    "website":"Official website URL.","wikipedia":"Wikipedia URL.",
    "fundref":"Preferred Crossref Funder ID. Join key to DataCite funding references.",
    "grid":"Preferred GRID id. Join key to legacy GRID-based org ids (e.g. in ORCID affiliations).",
    "isni":"Preferred ISNI.","wikidata":"Preferred Wikidata QID. Join key to Wikidata.",
    "n_relationships":"Number of related organizations.","n_names":"Number of name variants.",
    "created_date":"Registry creation date.","last_modified_date":"Registry last-modified date.",
    "types_json":"JSON array of all ROR types.","names_json":"JSON array of names ({value, types, lang}).",
    "locations_json":"JSON array of locations (geonames_id + details).",
    "links_json":"JSON array of links (website, wikipedia).",
    "external_ids_json":"JSON array of external ids ({type, all, preferred}: fundref/grid/isni/wikidata).",
    "relationships_json":"JSON array of relationships ({type: parent/child/related/predecessor/successor, id, label}).",
    "domains_json":"JSON array of associated domains.",
}
JSON_T = {"string":"string","integer":"integer","number":"number"}
CR_T = {"string":"sc:Text","integer":"sc:Integer","number":"sc:Float"}
LICENSE = "https://creativecommons.org/publicdomain/zero/1.0/"

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


def main():
    props = {}
    for c, t in COLS:
        typ = JSON_T[t] if c == "ror_id" else [JSON_T[t], "null"]
        p = {"type": typ, "description": DESC[c]}
        if c.endswith("_json"):
            p["contentMediaType"] = "application/json"
        props[c] = p
    schema = {
        "$schema":"https://json-schema.org/draft/2020-12/schema",
        "$id":"https://w3id.org/rete/ror/schema.json",
        "title":"ROR — Parquet row schema",
        "description":"One object = one research organization from the ROR registry "
                      "(schema v2) converted to Parquet. Source: https://ror.org (CC0).",
        "type":"object","properties":props,"required":["ror_id"],"additionalProperties":False,
    }
    with open(os.path.join(OUT,"schema.json"),"w",encoding="utf-8") as f:
        json.dump(schema,f,indent=2,ensure_ascii=False)

    fields=[{"@type":"cr:Field","@id":f"organization/{c}","name":c,"description":DESC[c],
             "dataType":CR_T[t],"source":{"fileSet":{"@id":"parquet_files"},"extract":{"column":c}}}
            for c,t in COLS]
    croissant={
        "@context":CR_CONTEXT,"@type":"sc:Dataset","conformsTo":"http://mlcommons.org/croissant/1.0",
        "name":"ror-research-organization-registry","description":"The Research Organization "
            "Registry (ROR) — ~111k research organizations with ROR IDs, names, locations, types, "
            "relationships and external ids (GRID, ISNI, Wikidata, Crossref Funder ID) — converted "
            "to Parquet. The organization authority that resolves the org ids used across ORCID, "
            "DataCite and EPFL.",
        "version":"1.54","datePublished":"2024-10-21","license":LICENSE,"url":"https://ror.org",
        "sameAs":"https://doi.org/10.5281/zenodo.13965926",
        "citeAs":"ROR — Research Organization Registry (v1.54, 2024-10-21). https://ror.org (CC0).",
        "keywords":["organizations","ROR","research institutions","persistent identifiers","GRID","funders"],
        "creator":{"@type":"sc:Organization","name":"ROR (Research Organization Registry)","url":"https://ror.org"},
        "distribution":[{"@type":"cr:FileSet","@id":"parquet_files","name":"parquet_files",
            "description":"Parquet file of the ROR registry.","encodingFormat":"application/x-parquet",
            "includes":"parquet/ror.parquet"}],
        "recordSet":[{"@type":"cr:RecordSet","@id":"organization","name":"organization",
            "description":f"One research organization per row ({N_ROWS:,} rows).",
            "key":{"@id":"organization/ror_id"},"field":fields}],
    }
    with open(os.path.join(OUT,"croissant.jsonld"),"w",encoding="utf-8") as f:
        json.dump(croissant,f,indent=2,ensure_ascii=False)
    print(f"wrote schema.json ({len(COLS)} fields) + croissant.jsonld")


if __name__ == "__main__":
    main()
