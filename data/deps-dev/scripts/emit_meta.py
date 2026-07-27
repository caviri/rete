#!/usr/bin/env python3
"""Emit schema.json + croissant.jsonld for deps-dev from the KNOWN schemas.

No Parquet reads: our files have tens of thousands of row groups, so pyarrow's
footer parse (read_schema / .metadata) is grindingly slow. The exact schemas were
captured with DuckDB DESCRIBE and the row counts from the .done markers, so we
encode them directly here — instant and reproducible. Distribution points at R2.
"""
import json

OUT = "/w/data/deps-dev"
R2 = "https://data.graphplaza.com/deps-dev/raw"
NAME, VERSION, DATE = "deps-dev", "1.0.0", "2026-07-13"
LICENSE = "https://creativecommons.org/licenses/by/4.0/"
DESC = ("deps.dev (Open Source Insights) snapshot 2026-07-13: package versions, "
        "version-pinned dependency edges, package-project links, projects, and "
        "security advisories. Google, CC BY 4.0.")

# type DSL: "str" "int" "num" "bool" "ts"; ("list", inner); ("struct", {name: t})
S, I, N, B, T = "str", "int", "num", "bool", "ts"

def L(inner):    return ("list", inner)
def ST(**flds):  return ("struct", flds)

TABLES = {
  "PackageVersions": (161888666, {
     "SnapshotAt": T, "System": S, "Name": S, "Version": S,
     "Licenses": L(S),
     "Links": L(ST(Label=S, URL=S)),
     "Advisories": L(ST(Source=S, SourceID=S)),
     "VersionInfo": ST(IsRelease=B, Ordinal=I),
     "Hashes": L(ST(Type=S, Hash=S)),
     "DependenciesProcessed": B, "DependencyError": B,
     "UpstreamPublishedAt": T, "Registries": L(S),
     "SLSAProvenance": ST(SourceRepository=S, Commit=S, URL=S, Verified=B),
     "UpstreamIdentifiers": L(ST(PackageName=S, VersionString=S, Source=S)),
     "Purl": S,
     "Attestations": L(ST(URL=S, Type=S, SourceRepository=S, Commit=S, Verified=B)),
     "Description": S, "Deprecated": S, "ProjectStatus": S, "ProjectStatusReason": S}),
  "dependency_edges": (570601975, {
     "System": S, "from_name": S, "from_version": S,
     "dep_system": S, "to_name": S, "to_version": S}),
  "PackageVersionToProject": (172093192, {
     "SnapshotAt": T, "System": S, "Name": S, "Version": S,
     "ProjectType": S, "ProjectName": S, "RelationProvenance": S, "RelationType": S}),
  "Projects": (5122936, {
     "SnapshotAt": T, "Type": S, "Name": S,
     "OpenIssuesCount": I, "StarsCount": I, "ForksCount": I,
     "Licenses": L(S), "Description": S, "Homepage": S,
     "OSSFuzz": ST(LineCount=I, LineCoverCount=I, Date=T, ConfigURL=S)}),
  "Advisories": (272582, {
     "SnapshotAt": T, "Source": S, "SourceID": S, "SourceURL": S,
     "Title": S, "Description": S, "ReferenceURLs": L(S),
     "CVSS3Score": N, "Severity": S, "GitHubSeverity": S, "Disclosed": T,
     "Packages": L(ST(System=S, Name=S, AffectedVersions=S, UnaffectedVersions=S)),
     "Aliases": L(S)}),
}

CTX = {
    "@language": "en", "@vocab": "https://schema.org/",
    "citeAs": "cr:citeAs", "column": "cr:column", "conformsTo": "dct:conformsTo",
    "cr": "http://mlcommons.org/croissant/",
    "data": {"@id": "cr:data", "@type": "@json"},
    "dataType": {"@id": "cr:dataType", "@type": "@vocab"},
    "dct": "http://purl.org/dc/terms/", "equivalentProperty": "cr:equivalentProperty",
    "examples": {"@id": "cr:examples", "@type": "@json"},
    "extract": "cr:extract", "field": "cr:field", "fileObject": "cr:fileObject",
    "fileProperty": "cr:fileProperty", "fileSet": "cr:fileSet", "format": "cr:format",
    "includes": "cr:includes", "isLiveDataset": "cr:isLiveDataset",
    "jsonPath": "cr:jsonPath", "key": "cr:key", "md5": "cr:md5",
    "parentField": "cr:parentField", "path": "cr:path",
    "rai": "http://mlcommons.org/croissant/RAI/",
    "recordSet": "cr:recordSet", "references": "cr:references", "regex": "cr:regex",
    "repeated": "cr:repeated", "replace": "cr:replace",
    "samplingRate": "cr:samplingRate", "sc": "https://schema.org/",
    "separator": "cr:separator", "source": "cr:source", "subField": "cr:subField",
    "transform": "cr:transform",
}

JS = {"str": "string", "ts": "string", "int": "integer", "num": "number", "bool": "boolean"}
CR = {"str": "sc:Text", "ts": "sc:Text", "int": "sc:Integer", "num": "sc:Float", "bool": "sc:Boolean"}


def js(t):
    if isinstance(t, tuple):
        if t[0] == "list":
            return {"type": "array", "items": js(t[1])}
        return {"type": "object", "properties": {k: js(v) for k, v in t[1].items()}}
    return {"type": JS[t]}


def cr(t):
    while isinstance(t, tuple):
        t = t[1] if t[0] == "list" else "str"   # struct -> text (croissant has no struct)
    return CR[t]


defs, dist, rsets = {}, [], []
total = 0
for tname, (rows, cols) in TABLES.items():
    total += rows
    defs[tname] = {"type": "object", "title": tname, "description": f"{rows:,} rows.",
                   "properties": {k: js(v) for k, v in cols.items()},
                   "additionalProperties": False}
    fid = f"file-{tname}"
    dist.append({"@type": "cr:FileObject", "@id": fid, "name": f"{tname}.parquet",
                 "description": f"{tname} Parquet ({rows:,} rows).",
                 "contentUrl": f"{R2}/{tname}.parquet",
                 "encodingFormat": "application/x-parquet"})
    rsets.append({"@type": "cr:RecordSet", "@id": tname, "name": tname,
                  "description": f"deps-dev {tname} records ({rows:,}).",
                  "field": [dict({"@type": "cr:Field", "@id": f"{tname}/{k}", "name": k,
                                  "dataType": cr(v),
                                  "source": {"fileObject": {"@id": fid},
                                             "extract": {"column": k}}},
                                 **({"repeated": True}
                                    if isinstance(v, tuple) and v[0] == "list" else {}))
                            for k, v in cols.items()]})

schema_doc = {"$schema": "https://json-schema.org/draft/2020-12/schema",
              "$id": f"https://w3id.org/rete/{NAME}/schema.json",
              "title": f"{NAME} — Parquet table schemas", "description": DESC, "$defs": defs}
croissant = {"@context": CTX, "@type": "Dataset",
             "conformsTo": "http://mlcommons.org/croissant/1.0",
             "name": NAME, "description": DESC, "url": "https://deps.dev/",
             "license": LICENSE, "version": VERSION, "datePublished": DATE,
             "citeAs": "Includes data from deps.dev (Open Source Insights) by Google, CC BY 4.0.",
             "distribution": dist, "recordSet": rsets}

json.dump(schema_doc, open(f"{OUT}/schema.json", "w", encoding="utf-8"), indent=1, ensure_ascii=False)
json.dump(croissant, open(f"{OUT}/croissant.jsonld", "w", encoding="utf-8"), indent=1, ensure_ascii=False)
print(f"wrote schema.json ({len(defs)} tables, {total:,} rows) + croissant.jsonld")

from jsonschema import Draft202012Validator
Draft202012Validator.check_schema(schema_doc)
print("schema.json: VALID (draft 2020-12)")
