"""Generate a JSON Schema and an MLCommons Croissant (1.0) JSON-LD description
for the Zenodo Parquet datasets (metadata + biosyslit + the two deleted
ledgers). Row counts are read from the Parquet footers so they are always
exact — run this after the conversions finish.

Writes:
  data/zenodo/schema.json       JSON Schema (draft 2020-12): 4 $defs
  data/zenodo/croissant.jsonld  Croissant 1.0: 4 FileSets + 4 RecordSets
"""

import glob
import json
import os

import pyarrow.parquet as pq

OUT = r"D:\pro\rete\data\zenodo"
LICENSE = "https://creativecommons.org/licenses/by/4.0/"  # Zenodo metadata export: CC-BY 4.0

# --- column lists (name, duckdb type) mirroring each converter's Arrow schema ---
METADATA = [
    ("doi", "VARCHAR"), ("record_id", "VARCHAR"), ("prefix", "VARCHAR"),
    ("publisher", "VARCHAR"), ("publication_year", "INTEGER"),
    ("published", "VARCHAR"), ("updated", "VARCHAR"),
    ("resource_type_general", "VARCHAR"), ("resource_type", "VARCHAR"),
    ("title", "VARCHAR"), ("language", "VARCHAR"), ("version", "VARCHAR"),
    ("schema_version", "VARCHAR"), ("url", "VARCHAR"),
    ("types_json", "VARCHAR"), ("creators_json", "VARCHAR"),
    ("titles_json", "VARCHAR"), ("subjects_json", "VARCHAR"),
    ("contributors_json", "VARCHAR"), ("dates_json", "VARCHAR"),
    ("related_identifiers_json", "VARCHAR"), ("descriptions_json", "VARCHAR"),
    ("geo_locations_json", "VARCHAR"), ("funding_references_json", "VARCHAR"),
    ("rights_list_json", "VARCHAR"), ("alternate_identifiers_json", "VARCHAR"),
    ("sizes_json", "VARCHAR"), ("formats_json", "VARCHAR"), ("extra_json", "VARCHAR"),
]
BIOSYSLIT = [
    ("doi", "VARCHAR"), ("record_id", "VARCHAR"), ("parent_id", "VARCHAR"),
    ("parent_doi", "VARCHAR"), ("created", "VARCHAR"), ("updated", "VARCHAR"),
    ("publication_date", "VARCHAR"), ("publisher", "VARCHAR"),
    ("resource_type_id", "VARCHAR"), ("resource_type_title", "VARCHAR"),
    ("title", "VARCHAR"), ("is_published", "BOOLEAN"), ("access_status", "VARCHAR"),
    ("communities", "VARCHAR"), ("views", "BIGINT"), ("unique_views", "BIGINT"),
    ("downloads", "BIGINT"), ("unique_downloads", "BIGINT"),
    ("file_count", "INTEGER"), ("total_bytes", "BIGINT"), ("description", "VARCHAR"),
    ("creators_json", "VARCHAR"), ("subjects_json", "VARCHAR"),
    ("identifiers_json", "VARCHAR"), ("related_identifiers_json", "VARCHAR"),
    ("rights_json", "VARCHAR"), ("additional_descriptions_json", "VARCHAR"),
    ("references_json", "VARCHAR"), ("custom_fields_json", "VARCHAR"),
    ("files_json", "VARCHAR"), ("pids_json", "VARCHAR"),
    ("iiif_manifest", "VARCHAR"), ("extra_json", "VARCHAR"),
]
DELETED = [
    ("record_id", "VARCHAR"), ("doi", "VARCHAR"), ("parent_id", "VARCHAR"),
    ("parent_doi", "VARCHAR"), ("removal_note", "VARCHAR"),
    ("removal_reason", "VARCHAR"), ("removal_date", "VARCHAR"),
    ("citation_text", "VARCHAR"),
]

DESC = {
    "doi": "DOI of the record. Primary cross-dataset join key (DataCite / OpenCitations / OpenAIRE / ORCID / DBLP); lowercase on join.",
    "record_id": "Zenodo record id (recid). Every version of a deposit has its own recid; the landing page is https://zenodo.org/records/<record_id>.",
    "prefix": "DOI prefix (registrant namespace); 10.5281 for Zenodo-minted DOIs.",
    "publisher": "Publisher name (Zenodo, or the original publisher for imported DOIs).",
    "publication_year": "Publication year.",
    "published": "Issued date (DataCite date[@dateType=Issued]).",
    "updated": "Last-updated date (DataCite date[@dateType=Updated]).",
    "resource_type_general": "DataCite general resource type (Text, Dataset, Software, Image, Preprint, JournalArticle, …).",
    "resource_type": "Free-text resource type as supplied.",
    "title": "First title (convenience; full list in titles_json).",
    "language": "Primary language of the resource.",
    "version": "Version label of the resource.",
    "schema_version": "DataCite metadata schema version of the export (4.5).",
    "url": "Landing-page URL (https://zenodo.org/records/<record_id>).",
    "types_json": "JSON object of the DataCite types block (resourceTypeGeneral, resourceType).",
    "creators_json": "JSON array of creators: name, givenName, familyName, nameIdentifiers (ORCID), affiliation.",
    "titles_json": "JSON array of all titles.",
    "subjects_json": "JSON array of subjects/keywords.",
    "contributors_json": "JSON array of contributors with contributorType.",
    "dates_json": "JSON array of typed dates.",
    "related_identifiers_json": "JSON array of related identifiers: relationType, relatedIdentifier, relatedIdentifierType. The citation/version network.",
    "descriptions_json": "JSON array of descriptions/abstracts.",
    "geo_locations_json": "JSON array of geo locations.",
    "funding_references_json": "JSON array of funding references (funder, award number).",
    "rights_list_json": "JSON array of rights/licenses (rights, rightsUri, rightsIdentifier, SPDX scheme).",
    "alternate_identifiers_json": "JSON array of alternate identifiers (URL, OAI id).",
    "sizes_json": "JSON array of sizes.",
    "formats_json": "JSON array of formats.",
    "extra_json": "JSON catch-all of fields not promoted to a column (e.g. datacentreSymbol).",
    # biosyslit-specific
    "parent_id": "Zenodo concept (parent) record id shared across versions.",
    "parent_doi": "Concept DOI shared across all versions of the deposit.",
    "created": "Record creation timestamp (ISO 8601).",
    "publication_date": "Publication date string as supplied.",
    "resource_type_id": "Zenodo resource type id (e.g. publication-taxonomictreatment, publication-article).",
    "resource_type_title": "English label of the Zenodo resource type.",
    "is_published": "Whether the record is published.",
    "access_status": "Access status (open, embargoed, restricted, closed).",
    "communities": "JSON array of Zenodo community slugs the record belongs to (always includes biosyslit).",
    "views": "Total views across all versions.",
    "unique_views": "Unique views across all versions.",
    "downloads": "Total downloads across all versions.",
    "unique_downloads": "Unique downloads across all versions.",
    "file_count": "Number of files attached to the record.",
    "total_bytes": "Total size of the record's files in bytes.",
    "description": "Primary description/abstract (HTML as supplied).",
    "identifiers_json": "JSON array of the record's own alternate identifiers (e.g. Plazi treatment URL).",
    "rights_json": "JSON array of rights/licenses (Zenodo REST shape).",
    "additional_descriptions_json": "JSON array of additional descriptions with types.",
    "references_json": "JSON array of cited references.",
    "custom_fields_json": "JSON object of custom fields — Darwin Core taxonomy (dwc:kingdom/phylum/class/order/family/genus, dwc:taxonRank, dwc:scientificNameAuthorship) and journal:journal.",
    "files_json": "JSON object of file entries (key, checksum, size, mimetype).",
    "pids_json": "JSON object of persistent identifiers (doi provider/client, oai id).",
    "iiif_manifest": "IIIF Presentation manifest URL for the record's images.",
    # deleted
    "removal_note": "Free-text note about the removal.",
    "removal_reason": "Reason code for removal (spam, take-down, duplicate, …).",
    "removal_date": "Date the record was removed (YYYY-MM-DD).",
    "citation_text": "Tombstone citation text, when preserved.",
}

RS = {
    "metadata": {
        "glob": "parquet-metadata/part-*.parquet", "cols": METADATA, "key": "doi",
        "blurb": "One row per Zenodo record (DataCite kernel-4.5 metadata), UNION-compatible with the DataCite Parquet and joinable on doi.",
    },
    "biosyslit": {
        "glob": "parquet-biosyslit/part-*.parquet", "cols": BIOSYSLIT, "key": "doi",
        "blurb": "Biodiversity Literature Repository community records (Zenodo-native JSON) with file/IIIF links, usage stats and Darwin Core taxonomy.",
    },
    "deleted": {
        "glob": "records-deleted.parquet", "cols": DELETED, "key": "record_id",
        "blurb": "Site-wide deletion ledger: records removed from Zenodo, with reason and date.",
    },
    "biosyslit_deleted": {
        "glob": "biosyslit-records-deleted.parquet", "cols": DELETED, "key": "record_id",
        "blurb": "Deletion ledger for the Biodiversity Literature Repository community.",
    },
}

JSON_T = {"VARCHAR": "string", "INTEGER": "integer", "BIGINT": "integer", "BOOLEAN": "boolean"}
CR_T = {"VARCHAR": "sc:Text", "INTEGER": "sc:Integer", "BIGINT": "sc:Integer", "BOOLEAN": "sc:Boolean"}


def count_rows(pattern):
    total = 0
    for p in sorted(glob.glob(os.path.join(OUT, pattern))):
        try:
            total += pq.ParquetFile(p).metadata.num_rows
        except Exception:  # noqa: BLE001
            pass
    return total


def props(cols, required):
    out = {}
    for c, dt in cols:
        typ = JSON_T[dt] if c in required else [JSON_T[dt], "null"]
        p = {"type": typ, "description": DESC[c]}
        if c.endswith("_json") or c in ("communities",):
            p["contentMediaType"] = "application/json"
        out[c] = p
    return out


def json_schema():
    defs = {}
    for name, spec in RS.items():
        defs[name] = {
            "type": "object", "title": f"Zenodo {name} record",
            "description": spec["blurb"],
            "properties": props(spec["cols"], {spec["key"]}),
            "required": [spec["key"]], "additionalProperties": False,
        }
    return {
        "$schema": "https://json-schema.org/draft/2020-12/schema",
        "$id": "https://w3id.org/rete/zenodo/schema.json",
        "title": "Zenodo exporter dumps — Parquet row schemas",
        "description": "Row schemas for the Zenodo full-repository exporter dumps "
                       "(https://zenodo.org/api/exporter) converted to Parquet. "
                       "'metadata' = one DataCite record per Zenodo recid; 'biosyslit' = "
                       "the Biodiversity Literature Repository community as Zenodo-native "
                       "JSON; 'deleted'/'biosyslit_deleted' = deletion ledgers. Metadata "
                       "is CC-BY 4.0.",
        "$defs": defs,
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


def croissant(counts):
    distribution, record_sets = [], []
    for name, spec in RS.items():
        fs = name + "_files"
        distribution.append({"@type": "cr:FileSet", "@id": fs, "name": fs,
            "description": f"Parquet shards for {name}.",
            "encodingFormat": "application/x-parquet", "includes": spec["glob"]})
        fields = [{"@type": "cr:Field", "@id": f"{name}/{c}", "name": c, "description": DESC[c],
                   "dataType": CR_T[dt], "source": {"fileSet": {"@id": fs}, "extract": {"column": c}}}
                  for c, dt in spec["cols"]]
        record_sets.append({"@type": "cr:RecordSet", "@id": name, "name": name,
            "description": f"{spec['blurb']} — {counts[name]:,} rows.", "field": fields})
    return {
        "@context": CR_CONTEXT, "@type": "sc:Dataset", "conformsTo": "http://mlcommons.org/croissant/1.0",
        "name": "zenodo-exporter-dumps",
        "description": "The Zenodo full-repository exporter dumps converted to Parquet: "
            "DataCite kernel-4.5 metadata for every Zenodo record, the Biodiversity "
            "Literature Repository community as Zenodo-native JSON (with IIIF and Darwin "
            "Core taxonomy), and the deletion ledgers. Records carry the DOI, resource "
            "type, creators (with ORCIDs), and the related-identifier network, so Zenodo "
            "joins the DataCite/OpenAIRE/ORCID/DBLP/OpenCitations research graph on doi.",
        "version": "1.0.0", "datePublished": "2026-07-17",
        "license": LICENSE, "url": "https://zenodo.org/api/exporter",
        "citeAs": "Zenodo (2026). Zenodo exporter data files. https://zenodo.org/api/exporter",
        "keywords": ["DOI", "research data", "open access", "citations", "persistent identifiers",
                     "biodiversity", "metadata", "Zenodo"],
        "creator": {"@type": "sc:Organization", "name": "Zenodo / CERN", "url": "https://zenodo.org"},
        "distribution": distribution, "recordSet": record_sets,
    }


def main():
    counts = {name: count_rows(spec["glob"]) for name, spec in RS.items()}
    with open(os.path.join(OUT, "schema.json"), "w", encoding="utf-8") as f:
        json.dump(json_schema(), f, indent=2, ensure_ascii=False)
    with open(os.path.join(OUT, "croissant.jsonld"), "w", encoding="utf-8") as f:
        json.dump(croissant(counts), f, indent=2, ensure_ascii=False)
    print("row counts:", {k: f"{v:,}" for k, v in counts.items()})
    print(f"schema.json: {len(RS)} $defs; croissant: {len(RS)} FileSets + {len(RS)} RecordSets")


if __name__ == "__main__":
    main()
