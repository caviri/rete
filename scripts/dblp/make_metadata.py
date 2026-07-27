"""JSON Schema + MLCommons Croissant (1.0) for the DBLP Parquet tables.

Writes:
  data/dblp/schema.json       JSON Schema (draft 2020-12): 'record' + 'authorship'
  data/dblp/croissant.jsonld  Croissant 1.0: 2 FileSets + 2 RecordSets (record key + authorship FK)
"""

import json
import os

OUT = r"D:\pro\rete\data\dblp"

RECORD = [
    ("key","VARCHAR"),("type","VARCHAR"),("mdate","VARCHAR"),("publtype","VARCHAR"),
    ("title","VARCHAR"),("year","INTEGER"),("venue","VARCHAR"),("volume","VARCHAR"),
    ("number","VARCHAR"),("pages","VARCHAR"),("publisher","VARCHAR"),("isbn","VARCHAR"),
    ("series","VARCHAR"),("doi","VARCHAR"),("url","VARCHAR"),("crossref","VARCHAR"),
    ("n_authors","INTEGER"),("authors_json","VARCHAR"),("editors_json","VARCHAR"),
    ("ee_json","VARCHAR"),
]
AUTHORSHIP = [
    ("key","VARCHAR"),("type","VARCHAR"),("year","INTEGER"),("pos","INTEGER"),
    ("author","VARCHAR"),("orcid","VARCHAR"),
]
COUNTS = {"record": 12751652, "authorship": 33755620}

DESC = {
    "key":"DBLP record key (e.g. journals/cacm/Codd70). Stable identifier / primary key of a record; the join key from authorship.",
    "type":"DBLP entry type: article, inproceedings, proceedings, book, incollection, phdthesis, mastersthesis, www (person/alias page), data.",
    "mdate":"Last modification date of the record (YYYY-MM-DD).",
    "publtype":"Publication subtype qualifier when present (e.g. informal, withdrawn, encyclopedia).",
    "title":"Title of the publication (markup flattened to text).",
    "year":"Publication year.",
    "venue":"Journal (for article) or booktitle (for inproceedings/incollection).",
    "volume":"Volume.","number":"Issue/number.","pages":"Page range.",
    "publisher":"Publisher.","isbn":"ISBN.","series":"Series name.",
    "doi":"DOI extracted from the <ee> electronic edition, bare form. Cross-dataset join key (DataCite / OpenAIRE / OpenCitations / ORCID).",
    "url":"DBLP db/ URL of the record.",
    "crossref":"Key of the containing proceedings/book (for inproceedings/incollection) — a record→record link.",
    "n_authors":"Number of authors on the record.",
    "authors_json":"JSON array of authors: [{name, orcid}] (name keeps DBLP's 0001-style disambiguation suffix).",
    "editors_json":"JSON array of editors: [{name, orcid}].",
    "ee_json":"JSON array of electronic-edition URLs (DOIs, publisher links).",
    # authorship
    "pos":"Author position (0-based order) within the record.",
    "author":"Author name string (with DBLP disambiguation suffix).",
    "orcid":"ORCID iD of the author when DBLP records one. Join key to the ORCID dataset.",
}

JSON_T = {"VARCHAR":"string","INTEGER":"integer"}
CR_T = {"VARCHAR":"sc:Text","INTEGER":"sc:Integer"}
LICENSE = "https://creativecommons.org/publicdomain/zero/1.0/"


def props(cols, req):
    out = {}
    for c, dt in cols:
        typ = JSON_T[dt] if c in req else [JSON_T[dt], "null"]
        p = {"type": typ, "description": DESC[c]}
        if c.endswith("_json"):
            p["contentMediaType"] = "application/json"
        out[c] = p
    return out


def json_schema():
    return {
        "$schema":"https://json-schema.org/draft/2020-12/schema",
        "$id":"https://w3id.org/rete/dblp/schema.json",
        "title":"DBLP — Parquet row schemas",
        "description":"Row schemas for the DBLP computer-science bibliography converted "
                      "to Parquet: 'record' (one publication) and 'authorship' (one "
                      "author↔record edge). Source: https://dblp.org/xml/ (CC0).",
        "$defs":{
            "record":{"type":"object","title":"DBLP record",
                "description":f"One publication per row ({COUNTS['record']:,} rows).",
                "properties":props(RECORD,{"key"}),"required":["key"],
                "additionalProperties":False},
            "authorship":{"type":"object","title":"DBLP authorship",
                "description":f"One (record, author) edge per row ({COUNTS['authorship']:,} rows).",
                "properties":props(AUTHORSHIP,{"key","author"}),"required":["key","author"],
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
    def fields(rs, cols):
        out = []
        for c, dt in cols:
            f = {"@type":"cr:Field","@id":f"{rs}/{c}","name":c,"description":DESC[c],
                 "dataType":CR_T[dt],"source":{"fileSet":{"@id":rs+"_files"},"extract":{"column":c}}}
            if rs == "authorship" and c == "key":
                f["references"] = {"field":{"@id":"record/key"}}
            out.append(f)
        return out
    dist = [{"@type":"cr:FileSet","@id":rs+"_files","name":rs+"_files",
             "description":f"Parquet shards for the {rs} table.",
             "encodingFormat":"application/x-parquet","includes":f"parquet/{rs}/part-*.parquet"}
            for rs in ("record","authorship")]
    rsets = [
        {"@type":"cr:RecordSet","@id":"record","name":"record",
         "description":f"One DBLP publication per row ({COUNTS['record']:,} rows).",
         "key":{"@id":"record/key"},"field":fields("record",RECORD)},
        {"@type":"cr:RecordSet","@id":"authorship","name":"authorship",
         "description":f"Author↔record edges ({COUNTS['authorship']:,} rows).",
         "field":fields("authorship",AUTHORSHIP)},
    ]
    return {
        "@context":CR_CONTEXT,"@type":"sc:Dataset","conformsTo":"http://mlcommons.org/croissant/1.0",
        "name":"dblp","description":"The DBLP computer-science bibliography converted to Parquet: "
            "publications and an author↔publication edge table. Records carry DOIs (join to DataCite/"
            "OpenAIRE/OpenCitations) and authorships carry ORCID iDs (join to ORCID) — the CS-focused "
            "vertex of the scholarly graph.",
        "version":"1.0.0","datePublished":"2026-07-01","license":LICENSE,"url":"https://dblp.org/",
        "citeAs":"dblp computer science bibliography. https://dblp.org/ (CC0).",
        "keywords":["bibliography","computer science","publications","co-authorship","DOI","ORCID"],
        "creator":{"@type":"sc:Organization","name":"dblp / Schloss Dagstuhl","url":"https://dblp.org"},
        "distribution":dist,"recordSet":rsets,
    }


def main():
    with open(os.path.join(OUT,"schema.json"),"w",encoding="utf-8") as f:
        json.dump(json_schema(),f,indent=2,ensure_ascii=False)
    with open(os.path.join(OUT,"croissant.jsonld"),"w",encoding="utf-8") as f:
        json.dump(croissant(),f,indent=2,ensure_ascii=False)
    print("wrote schema.json (record/authorship) + croissant.jsonld (2 FileSets/RecordSets)")


if __name__ == "__main__":
    main()
