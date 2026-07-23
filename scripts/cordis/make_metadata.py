"""JSON Schema + MLCommons Croissant (1.0) for the CORDIS per-class Parquet
tables. Introspects data/cordis/parquet/*.parquet (one table per EURIO class)."""

import glob
import json
import os
import duckdb

BASE = r"D:\pro\rete\data\cordis\parquet"
OUT = r"D:\pro\rete\data\cordis"

DUCK_JSON = {"BIGINT":"integer","INTEGER":"integer","HUGEINT":"integer","DOUBLE":"number",
             "FLOAT":"number","BOOLEAN":"boolean"}
DUCK_CR = {"BIGINT":"sc:Integer","INTEGER":"sc:Integer","HUGEINT":"sc:Integer","DOUBLE":"sc:Float",
           "FLOAT":"sc:Float","BOOLEAN":"sc:Boolean"}

DESC = {
    "subject":"IRI of the entity (EURIO s66: resource URL).",
    "rdf_types":"JSON array of all rdf:type IRIs of the entity (multi-typed).",
    "doi":"DOI of the result. Cross-dataset join key (DataCite/OpenAIRE/OpenCitations/DBLP).",
    "isResultOf":"IRI of the project this result came from.",
    "hasResult":"IRIs of the project's results.",
    "isFundedBy":"IRI of the funding scheme/grant.",
    "hasInvolvedParty":"IRIs of the organisation roles involved in the project.",
    "hasTotalCost":"IRI of the total-cost MonetaryAmount.",
    "title":"Title.","label":"rdfs:label.","abstract":"Abstract.","keyword":"Keyword(s).",
    "startDate":"Start date.","endDate":"End date.","projectStatus":"Project status (e.g. CLOSED, SIGNED).",
    "author":"Author list (literal).","journalTitle":"Journal title.","publishedYear":"Publication year.",
    "publisher":"Publisher.","issn":"ISSN.","isbn":"ISBN.","url":"URL.","identifier":"Source identifier.",
    "rcn":"CORDIS record control number.",
}


def desc(col):
    return DESC.get(col, f"EURIO property '{col}'." if col not in ("subject","rdf_types") else DESC[col])


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
LICENSE = "https://creativecommons.org/licenses/by/4.0/"


def main():
    d = duckdb.connect()
    tables = []
    for f in sorted(glob.glob(os.path.join(BASE, "*.parquet"))):
        cls = os.path.basename(f)[:-8]
        cols = [(r[0], r[1]) for r in d.execute(f"DESCRIBE SELECT * FROM read_parquet('{f.replace(chr(92),'/')}')").fetchall()]
        n = d.execute(f"SELECT count(*) FROM read_parquet('{f.replace(chr(92),'/')}')").fetchone()[0]
        tables.append((cls, n, cols))

    defs = {}
    for cls, n, cols in tables:
        props = {c: {"type": [DUCK_JSON.get(t, "string"), "null"], "description": desc(c)} for c, t in cols}
        defs[cls] = {"type":"object","title":cls,"description":f"{n:,} entities.",
                     "properties":props,"required":["subject"],"additionalProperties":False}
    schema = {
        "$schema":"https://json-schema.org/draft/2020-12/schema",
        "$id":"https://w3id.org/rete/cordis/schema.json",
        "title":"CORDIS (EURIO) — per-class Parquet row schemas",
        "description":"Row schemas for the CORDIS EURIO Knowledge Graph flattened to one "
                      "Parquet table per EURIO class. One $def per class; object-property "
                      "columns hold target IRIs. Source: EU Open Data Portal (CC-BY 4.0).",
        "$defs":defs,
    }
    with open(os.path.join(OUT,"schema.json"),"w",encoding="utf-8") as f:
        json.dump(schema,f,indent=2,ensure_ascii=False)

    dist, rsets = [], []
    for cls, n, cols in tables:
        fs = cls + "_files"
        dist.append({"@type":"cr:FileSet","@id":fs,"name":fs,"description":f"Parquet for {cls}.",
                     "encodingFormat":"application/x-parquet","includes":f"parquet/{cls}.parquet"})
        fields=[{"@type":"cr:Field","@id":f"{cls}/{c}","name":c,"description":desc(c),
                 "dataType":DUCK_CR.get(t,"sc:Text"),"source":{"fileSet":{"@id":fs},"extract":{"column":c}}}
                for c,t in cols]
        rs={"@type":"cr:RecordSet","@id":cls,"name":cls,"description":f"{n:,} entities.","field":fields}
        rsets.append(rs)
    croissant={
        "@context":CR_CONTEXT,"@type":"sc:Dataset","conformsTo":"http://mlcommons.org/croissant/1.0",
        "name":"cordis-eurio-knowledge-graph","description":"CORDIS — the EU's research projects "
            "(Horizon Europe, H2020, FP7…) as the EURIO Knowledge Graph, flattened to one Parquet "
            "table per class: projects, grants, results (with DOIs), organisations, funding schemes "
            "and their roles. The EU-funding layer of the scholarly graph.",
        "version":"1.0.0","datePublished":"2026-04-02","license":LICENSE,
        "url":"https://data.europa.eu/data/datasets/named-graphs-from-eurio-knowledge-graph",
        "citeAs":"CORDIS EURIO Knowledge Graph. European Commission / EU Open Data Portal (CC-BY 4.0).",
        "keywords":["CORDIS","EURIO","EU research","projects","grants","Horizon Europe","funding"],
        "creator":{"@type":"sc:Organization","name":"European Commission (CORDIS)","url":"https://cordis.europa.eu"},
        "distribution":dist,"recordSet":rsets,
    }
    with open(os.path.join(OUT,"croissant.jsonld"),"w",encoding="utf-8") as f:
        json.dump(croissant,f,indent=2,ensure_ascii=False)
    print(f"wrote schema.json + croissant.jsonld: {len(tables)} class tables")


if __name__ == "__main__":
    main()
