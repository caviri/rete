#!/usr/bin/env python3
"""Emit the metadata bundle for the wikidata-ontology dataset: JSON Schemas,
MLCommons Croissant JSON-LD, and a compact Turtle taxonomy.

Companions to scripts/wikidata_ontology_to_nt.py (run that first — it writes
`classes.parquet`, `class_counts.parquet`, `p279.parquet` next to the NT):

* `schemas/*.schema.json` — JSON Schema (2020-12) for each record set: the raw
  truthy triplet rows and the three ontology side-tables.
* `croissant.json` — Croissant 1.0 (https://mlcommons.org/croissant/): the
  source Parquet partitions on Hugging Face + the derived artifacts on R2, with
  RecordSets/Fields so HF `datasets`/TFDS can load them directly.
* `wikidata-taxonomy.ttl` — the distributable ontology file Wikidata itself
  never ships: every class as `rdfs:Class` with its English label, its
  `rdfs:subClassOf` edges, and `rete:instanceCount` (direct P31 instances in
  the full dump). Needs one DuckDB scan of the partitions for English labels.

Usage:
  python scripts/wikidata_ontology_metadata.py                  # everything
  python scripts/wikidata_ontology_metadata.py --only schemas,croissant
  python scripts/wikidata_ontology_metadata.py --only ttl
"""

from __future__ import annotations

import argparse
import glob
import json
import os
import sys
import time

HF_REPO = "https://huggingface.co/datasets/piebro/wikidata-extraction"
R2_BASE = "https://data.graphplaza.com/wikidata-ontology"
RETE_NS = "https://w3id.org/rete/"
RDFS_LABEL = "http://www.w3.org/2000/01/rdf-schema#label"

URI = {"type": "string", "format": "uri"}

SCHEMAS: dict[str, dict] = {
    "triplets": {
        "$schema": "https://json-schema.org/draft/2020-12/schema",
        "$id": f"{R2_BASE}/schemas/triplets.schema.json",
        "title": "Wikidata truthy triplet row",
        "description": "One row of the piebro/wikidata-extraction Parquet "
        "partitions: a truthy (wdt:) Wikidata statement. `object` holds an IRI "
        "or a literal lexical form; `language` is set for monolingual text.",
        "type": "object",
        "properties": {
            "subject": URI,
            "predicate": URI,
            "object": {"type": "string", "description": "IRI or literal lexical form"},
            "language": {"type": ["string", "null"], "description": "BCP-47 tag for monolingual text"},
        },
        "required": ["subject", "predicate", "object"],
    },
    "classes": {
        "$schema": "https://json-schema.org/draft/2020-12/schema",
        "$id": f"{R2_BASE}/schemas/classes.schema.json",
        "title": "Wikidata class",
        "description": "The class set of the Wikidata ontology: every item "
        "that is a wdt:P279 subject or object, or a wdt:P31 target.",
        "type": "object",
        "properties": {"cls": URI},
        "required": ["cls"],
    },
    "class_counts": {
        "$schema": "https://json-schema.org/draft/2020-12/schema",
        "$id": f"{R2_BASE}/schemas/class_counts.schema.json",
        "title": "Wikidata class instance count",
        "description": "Direct instance count per class: how many subjects "
        "have `wdt:P31 <cls>` in the full truthy dump.",
        "type": "object",
        "properties": {"cls": URI, "n": {"type": "integer", "minimum": 1}},
        "required": ["cls", "n"],
    },
    "p279": {
        "$schema": "https://json-schema.org/draft/2020-12/schema",
        "$id": f"{R2_BASE}/schemas/p279.schema.json",
        "title": "Wikidata subclass edge",
        "description": "A wdt:P279 (subclass of) edge: subject is a subclass "
        "of object.",
        "type": "object",
        "properties": {"subject": URI, "object": URI},
        "required": ["subject", "object"],
    },
}

# The standard Croissant 1.0 @context (per the MLCommons spec).
CR_CONTEXT = {
    "@language": "en",
    "@vocab": "https://schema.org/",
    "citeAs": "cr:citeAs",
    "column": "cr:column",
    "conformsTo": "dct:conformsTo",
    "cr": "http://mlcommons.org/croissant/",
    "rai": "http://mlcommons.org/croissant/RAI/",
    "data": {"@id": "cr:data", "@type": "@json"},
    "dataType": {"@id": "cr:dataType", "@type": "@vocab"},
    "dct": "http://purl.org/dc/terms/",
    "examples": {"@id": "cr:examples", "@type": "@json"},
    "extract": "cr:extract",
    "field": "cr:field",
    "fileProperty": "cr:fileProperty",
    "fileObject": "cr:fileObject",
    "fileSet": "cr:fileSet",
    "format": "cr:format",
    "includes": "cr:includes",
    "isLiveDataset": "cr:isLiveDataset",
    "jsonPath": "cr:jsonPath",
    "key": "cr:key",
    "md5": "cr:md5",
    "parentField": "cr:parentField",
    "path": "cr:path",
    "recordSet": "cr:recordSet",
    "references": "cr:references",
    "regex": "cr:regex",
    "repeated": "cr:repeated",
    "replace": "cr:replace",
    "sc": "https://schema.org/",
    "separator": "cr:separator",
    "source": "cr:source",
    "subField": "cr:subField",
    "transform": "cr:transform",
}


def _field(rs: str, name: str, dtype: str, source_ref: dict, desc: str = "") -> dict:
    f = {
        "@type": "cr:Field",
        "@id": f"{rs}/{name}",
        "name": name,
        "dataType": dtype,
        "source": {**source_ref, "extract": {"column": name}},
    }
    if desc:
        f["description"] = desc
    return f


def croissant_doc() -> dict:
    fs_parts = {"fileSet": {"@id": "parquet-partitions"}}

    def fo(name: str) -> dict:
        return {"fileObject": {"@id": name}}

    return {
        "@context": CR_CONTEXT,
        "@type": "sc:Dataset",
        "name": "wikidata-ontology",
        "conformsTo": "http://mlcommons.org/croissant/1.0",
        "description": (
            "The complete Wikidata class ontology derived from the truthy dump: "
            "4.4M classes (wdt:P279 subjects/objects and wdt:P31 targets), the "
            "5.1M-edge subclass hierarchy, per-class direct-instance counts, and "
            "the raw truthy triplet partitions it was derived from. Wikidata "
            "publishes no class-ontology file; this is that file, in Parquet, "
            "Turtle, N-Triples, and range-queryable .rete form."
        ),
        "license": "https://creativecommons.org/publicdomain/zero/1.0/",
        "url": "https://caviri.github.io/rete/",
        "citeAs": "Wikidata contributors, via piebro/wikidata-extraction (CC0)",
        "version": "1.0.0",
        "distribution": [
            {
                "@type": "cr:FileObject",
                "@id": "piebro-wikidata-extraction",
                "name": "piebro-wikidata-extraction",
                "description": "HF dataset repo with the truthy dump as Parquet.",
                "contentUrl": HF_REPO,
                "encodingFormat": "git+https",
                "sha256": "main",
            },
            {
                "@type": "cr:FileSet",
                "@id": "parquet-partitions",
                "name": "parquet-partitions",
                "description": "81 Parquet partitions of the Wikidata truthy dump "
                "(subject, predicate, object, language).",
                "containedIn": {"@id": "piebro-wikidata-extraction"},
                "encodingFormat": "application/x-parquet",
                "includes": "triplets/*.parquet",
            },
            *[
                {
                    "@type": "cr:FileObject",
                    "@id": key,
                    "name": key,
                    "description": desc,
                    "contentUrl": f"{R2_BASE}/{fname}",
                    "encodingFormat": fmt,
                }
                for key, fname, fmt, desc in [
                    ("classes", "classes.parquet", "application/x-parquet",
                     "The 4.4M-class set."),
                    ("class-counts", "class_counts.parquet", "application/x-parquet",
                     "Direct P31 instance count per instantiated class."),
                    ("p279-edges", "p279.parquet", "application/x-parquet",
                     "All wdt:P279 subclass edges."),
                    ("taxonomy-ttl", "wikidata-taxonomy.ttl", "text/turtle",
                     "Compact Turtle taxonomy: rdfs:Class + English label + "
                     "rdfs:subClassOf + rete:instanceCount per class."),
                    ("ontology-rete", "wikidata-ontology.rete", "application/octet-stream",
                     "Range-queryable .rete: the full multilingual star of every "
                     "class and property entity; SPARQL in the browser at "
                     "https://caviri.github.io/rete/playground/."),
                ]
            ],
        ],
        "recordSet": [
            {
                "@type": "cr:RecordSet",
                "@id": "triplets",
                "name": "triplets",
                "description": "Truthy Wikidata statements.",
                "field": [
                    _field("triplets", "subject", "sc:URL", fs_parts),
                    _field("triplets", "predicate", "sc:URL", fs_parts),
                    _field("triplets", "object", "sc:Text", fs_parts,
                           "IRI or literal lexical form"),
                    _field("triplets", "language", "sc:Text", fs_parts,
                           "BCP-47 tag for monolingual text, else null"),
                ],
            },
            {
                "@type": "cr:RecordSet",
                "@id": "classes",
                "name": "classes",
                "key": {"@id": "classes/cls"},
                "field": [_field("classes", "cls", "sc:URL", fo("classes"))],
            },
            {
                "@type": "cr:RecordSet",
                "@id": "class_counts",
                "name": "class_counts",
                "key": {"@id": "class_counts/cls"},
                "field": [
                    _field("class_counts", "cls", "sc:URL", fo("class-counts")),
                    _field("class_counts", "n", "sc:Integer", fo("class-counts"),
                           "direct P31 instances in the full dump"),
                ],
            },
            {
                "@type": "cr:RecordSet",
                "@id": "p279",
                "name": "p279",
                "field": [
                    _field("p279", "subject", "sc:URL", fo("p279-edges")),
                    _field("p279", "object", "sc:URL", fo("p279-edges")),
                ],
            },
        ],
    }


def ttl_escape(s: str) -> str:
    return (s.replace("\\", "\\\\").replace('"', '\\"')
            .replace("\n", "\\n").replace("\r", "\\r").replace("\t", "\\t"))


def emit_ttl(out_dir: str, local_dir: str, memory_limit: str) -> None:
    """One scan for English labels, then stream the taxonomy as Turtle."""
    import duckdb

    parts = sorted(glob.glob(os.path.join(local_dir, "part_*.parquet")))
    if not parts:
        sys.exit(f"no part_*.parquet in {local_dir}")
    for f in ("classes.parquet", "class_counts.parquet", "p279.parquet"):
        if not os.path.exists(os.path.join(out_dir, f)):
            sys.exit(f"{f} missing in {out_dir} — run wikidata_ontology_to_nt.py first")

    con = duckdb.connect()
    con.execute("SET enable_progress_bar=false")
    con.execute(f"SET memory_limit='{memory_limit}'")
    con.execute(f"SET temp_directory='{os.path.join(out_dir, 'duckdb_tmp')}'")
    src = "[" + ", ".join(f"'{p}'" for p in parts) + "]"
    cls_pq = os.path.join(out_dir, "classes.parquet").replace("\\", "/")
    cnt_pq = os.path.join(out_dir, "class_counts.parquet").replace("\\", "/")
    p279_pq = os.path.join(out_dir, "p279.parquet").replace("\\", "/")

    t0 = time.time()
    print(f"scanning {len(parts)} partitions for English class labels…", file=sys.stderr)
    con.execute(
        "CREATE TEMP TABLE en_labels AS "
        "SELECT subject, any_value(object) AS label "
        f"FROM read_parquet({src}) "
        f"WHERE predicate = '{RDFS_LABEL}' AND language = 'en' "
        f"  AND subject IN (SELECT cls FROM '{cls_pq}') "
        "GROUP BY subject"
    )
    n_lab = con.execute("SELECT count(*) FROM en_labels").fetchone()[0]
    print(f"  {n_lab:,} English labels ({time.time() - t0:.0f}s)", file=sys.stderr)

    cur = con.execute(
        "SELECT c.cls, l.label, cc.n, list(p.object ORDER BY length(p.object), p.object) "
        f"FROM '{cls_pq}' c "
        "LEFT JOIN en_labels l ON l.subject = c.cls "
        f"LEFT JOIN '{cnt_pq}' cc ON cc.cls = c.cls "
        f"LEFT JOIN '{p279_pq}' p ON p.subject = c.cls "
        "GROUP BY c.cls, l.label, cc.n "
        "ORDER BY length(c.cls), c.cls"
    )
    out = os.path.join(out_dir, "wikidata-taxonomy.ttl")
    wd = "http://www.wikidata.org/entity/"
    n_cls = 0
    with open(out, "w", encoding="utf-8", newline="\n") as f:
        f.write(
            "@prefix wd: <http://www.wikidata.org/entity/> .\n"
            "@prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .\n"
            "@prefix owl: <http://www.w3.org/2002/07/owl#> .\n"
            "@prefix rete: <https://w3id.org/rete/> .\n\n"
            "<https://w3id.org/rete/wikidata-taxonomy> a owl:Ontology ;\n"
            '    rdfs:label "Wikidata class taxonomy"@en ;\n'
            '    rdfs:comment "Every Wikidata class (wdt:P279 subject/object or '
            "wdt:P31 target) with its English label, subclass edges, and direct-"
            'instance count. Derived from the truthy dump; CC0."@en .\n\n'
        )
        while True:
            batch = cur.fetchmany(100_000)
            if not batch:
                break
            lines = []
            for cls, label, n, supers in batch:
                if not cls.startswith(wd):
                    continue
                stmts = ["a rdfs:Class"]
                if label:
                    stmts.append(f'rdfs:label "{ttl_escape(label)}"@en')
                supers = [s for s in (supers or []) if s and s.startswith(wd)]
                if supers:
                    stmts.append("rdfs:subClassOf " +
                                 ", ".join(f"wd:{s[len(wd):]}" for s in supers))
                if n:
                    stmts.append(f"rete:instanceCount {n}")
                lines.append(f"wd:{cls[len(wd):]} " + " ;\n    ".join(stmts) + " .\n")
            f.write("".join(lines))
            n_cls += len(lines)
    gb = os.path.getsize(out) / 1e9
    print(f"wrote {n_cls:,} classes ({gb:.2f} GB) to {out} "
          f"in {time.time() - t0:.0f}s", file=sys.stderr)


def main() -> None:
    ap = argparse.ArgumentParser(
        description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter
    )
    ap.add_argument("--out-dir", default="data/wikidata-ontology")
    ap.add_argument("--local-dir", default="data/wikidata-parquet/triplets")
    ap.add_argument("--only", default="schemas,croissant,ttl",
                    help="comma list of: schemas, croissant, ttl")
    ap.add_argument("--memory-limit", default="16GB")
    args = ap.parse_args()
    only = {s.strip() for s in args.only.split(",")}

    if "schemas" in only:
        sdir = os.path.join(args.out_dir, "schemas")
        os.makedirs(sdir, exist_ok=True)
        for name, schema in SCHEMAS.items():
            path = os.path.join(sdir, f"{name}.schema.json")
            with open(path, "w", encoding="utf-8", newline="\n") as f:
                json.dump(schema, f, indent=2)
                f.write("\n")
            print(f"wrote {path}", file=sys.stderr)

    if "croissant" in only:
        os.makedirs(args.out_dir, exist_ok=True)
        path = os.path.join(args.out_dir, "croissant.json")
        with open(path, "w", encoding="utf-8", newline="\n") as f:
            json.dump(croissant_doc(), f, indent=2)
            f.write("\n")
        print(f"wrote {path}", file=sys.stderr)

    if "ttl" in only:
        emit_ttl(args.out_dir, args.local_dir, args.memory_limit)


if __name__ == "__main__":
    main()
