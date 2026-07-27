"""JSON Schema + MLCommons Croissant (1.0) for the EPFL GraphOntology Parquet
tables. Introspects the Parquet (DESCRIBE + count) so it stays correct across
all 18 non-empty tables (including the 104-column PageProfile).

Writes:
  data/epfl-graph/schema.json       JSON Schema (draft 2020-12), one $def per table
  data/epfl-graph/croissant.jsonld  Croissant 1.0, one FileSet + RecordSet per table
"""

import glob
import json
import os

import duckdb

BASE = r"D:\pro\rete\data\epfl-graph\parquet"
OUT = r"D:\pro\rete\data\epfl-graph"

DUCK_JSON = {"BIGINT":"integer","INTEGER":"integer","HUGEINT":"integer",
             "DOUBLE":"number","FLOAT":"number","BOOLEAN":"boolean"}
DUCK_CR = {"BIGINT":"sc:Integer","INTEGER":"sc:Integer","HUGEINT":"sc:Integer",
           "DOUBLE":"sc:Float","FLOAT":"sc:Float","BOOLEAN":"sc:Boolean"}

COMMON = {
    "institution_id":"Source institution id (constant 'Ont' for the ontology).",
    "object_type":"Node type of the object (Concept, Category, CuratedArea, …).",
    "object_id":"Identifier of the object within the graph.",
    "id":"Node identifier.","name":"Display name / label of the node.",
    "from_id":"Source node id of the edge.","to_id":"Target node id of the edge.",
    "score":"Edge weight / similarity score.","normalised_score":"Normalised edge score.",
    "embedding":"Vector embedding (serialized).","row_id":"Row sequence id from the source dump.",
    "depth":"Depth of the node in its hierarchy.",
    "reference_page_id":"Wikipedia reference page id.","reference_page_key":"Wikipedia reference page key.",
    "reference_page_url":"Wikipedia reference page URL.",
    "is_ontology_category":"Flag: node is an ontology category.",
    "is_ontology_concept":"Flag: node is an ontology concept.",
    "is_ontology_neighbour":"Flag: node is an ontology neighbour.",
    "is_noise":"Flag: node marked as noise.","is_unused":"Flag: node unused.",
    "field_language":"Language of the custom field.","field_name":"Custom field name.",
    "field_value":"Custom field value.","record_created_date":"Record creation timestamp.",
    "record_updated_date":"Record update timestamp.","date_created":"Creation date.",
    "date_terminated":"Termination date.","subtype":"Node subtype.","institution":"Institution.",
    "context":"Context.","url":"URL.","description":"Description.",
    "category_id":"Category id.","category_name":"Category name.","topic_id":"OpenAlex topic id.",
    "topic_name":"OpenAlex topic name.","embedding_score":"Embedding-based alignment score.",
    "wikipedia_score":"Wikipedia-based alignment score.","short_code":"Short code.",
}


def describe(con, path):
    return [(r[0], r[1]) for r in
            con.execute(f"DESCRIBE SELECT * FROM read_parquet('{path}')").fetchall()]


def desc_for(col, table):
    if col in COMMON:
        return COMMON[col]
    if col.startswith("full_content_"):
        return f"Full page text ({col.split('_')[-1]}, {'HTML' if 'html' in col else 'plain'})."
    if col.startswith("numeric_id_"):
        return f"Numeric page id ({col.split('_')[-1]})."
    if col.startswith("subtype_"):
        return f"Subtype ({col.split('_')[-1]})."
    return f"Page-profile field '{col}'." if "PageProfile" in table else f"Field '{col}'."


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
LICENSE = "https://www.apache.org/licenses/LICENSE-2.0"


def main():
    con = duckdb.connect()
    tables = []
    for dd in sorted(glob.glob(os.path.join(BASE, "*"))):
        if not os.path.isdir(dd):
            continue
        files = glob.glob(os.path.join(dd, "*.parquet"))
        if not files:
            continue
        t = os.path.basename(dd)
        n = con.execute(f"SELECT count(*) FROM read_parquet('{dd}/*.parquet')").fetchone()[0]
        if n == 0:
            continue
        tables.append((t, n, describe(con, files[0])))

    # JSON Schema
    defs = {}
    for t, n, cols in tables:
        props = {}
        for c, dt in cols:
            jt = DUCK_JSON.get(dt, "string")
            props[c] = {"type": [jt, "null"], "description": desc_for(c, t)}
        defs[t] = {"type":"object","title":t,"description":f"{n:,} rows.",
                   "properties":props,"additionalProperties":False}
    schema = {
        "$schema":"https://json-schema.org/draft/2020-12/schema",
        "$id":"https://w3id.org/rete/epfl-graph/schema.json",
        "title":"EPFL GraphOntology — Parquet table schemas",
        "description":"Row schemas for the EPFL GraphOntology (Wikipedia-derived concept "
                      "graph) converted to Parquet. One $def per table. Source: "
                      "https://zenodo.org/records/20306788 (Apache-2.0).",
        "$defs":defs,
    }
    with open(os.path.join(OUT,"schema.json"),"w",encoding="utf-8") as f:
        json.dump(schema,f,indent=2,ensure_ascii=False)

    # Croissant
    dist, rsets = [], []
    for t, n, cols in tables:
        fs = t + "_files"
        dist.append({"@type":"cr:FileSet","@id":fs,"name":fs,
            "description":f"Parquet shards for {t}.","encodingFormat":"application/x-parquet",
            "includes":f"parquet/{t}/*.parquet"})
        fields = [{"@type":"cr:Field","@id":f"{t}/{c}","name":c,"description":desc_for(c,t),
                   "dataType":DUCK_CR.get(dt,"sc:Text"),
                   "source":{"fileSet":{"@id":fs},"extract":{"column":c}}} for c, dt in cols]
        rsets.append({"@type":"cr:RecordSet","@id":t,"name":t,
            "description":f"{n:,} rows.","field":fields})
    croissant = {
        "@context":CR_CONTEXT,"@type":"sc:Dataset","conformsTo":"http://mlcommons.org/croissant/1.0",
        "name":"epfl-graphontology","description":"The EPFL GraphOntology — a Wikipedia-derived "
            "concept graph (concepts, categories, curated areas; directed/undirected/symmetric and "
            "embedding-similarity edges; multilingual page content and vector embeddings) — converted "
            "to Parquet.","version":"1.0.1","datePublished":"2025-06-26","license":LICENSE,
        "url":"https://www.epfl.ch/about/data/epfl-graph/","sameAs":"https://doi.org/10.5281/zenodo.20306788",
        "citeAs":"Pinto, F.C., Yazdanian, R. (2025). GraphOntology dataset. EPFL / Zenodo. "
                 "https://doi.org/10.5281/zenodo.20306788",
        "keywords":["knowledge graph","concepts","Wikipedia","embeddings","ontology","SKOS"],
        "creator":{"@type":"sc:Organization","name":"EPFL","url":"https://www.epfl.ch"},
        "distribution":dist,"recordSet":rsets,
    }
    with open(os.path.join(OUT,"croissant.jsonld"),"w",encoding="utf-8") as f:
        json.dump(croissant,f,indent=2,ensure_ascii=False)
    print(f"wrote schema.json + croissant.jsonld: {len(tables)} tables")


if __name__ == "__main__":
    main()
