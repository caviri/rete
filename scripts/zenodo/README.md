# Zenodo exporter dumps → Parquet

Converts the [Zenodo exporter data files](https://zenodo.org/api/exporter)
(full-repository snapshots, refreshed monthly) into queryable Parquet **without
ever extracting the archives** — tar members are streamed one at a time from the
compressed tar, parsed by a process pool, and written as rolling zstd Parquet.

The exporter publishes 3 rolling versions of each file; **every version is a
full snapshot, not a delta** (deletions are reconciled via the `records-deleted`
ledger). We download only the head versions (see `data/zenodo/`), immutable
versioned URLs, MD5-verified against the API. Metadata is **CC-BY 4.0**.

The site-wide `records-xml` records are DataCite kernel-4.5 XML, mapped to the
**same column set as `scripts/datacite/`**, so Zenodo `UNION BY NAME`s with the
DataCite Parquet and joins the whole research graph (DataCite / OpenAIRE / ORCID
/ DBLP / OpenCitations) on `doi`. See [[scholar-alignment]] for the hub ontology.

## Results

| Dataset | Rows | Output | Source |
|---|---|---|---|
| Metadata (all Zenodo) | ~10M | `parquet-metadata/`, ~zstd | `records-xml-2026-07-10.tar.gz` (5.76 GB / ~40 GB XML) |
| BLR community | ~0.8M | `parquet-biosyslit/` | `biosyslit-records-json-2026-03-27.tar.gz` (3.35 GB) |
| Deleted (site-wide) | 1,322,007 | `records-deleted.parquet` | `records-deleted-2026-07-10.csv.gz` |
| Deleted (BLR) | 344 | `biosyslit-records-deleted.parquet` | `biosyslit-records-deleted-2026-03-27.csv.gz` |

```
python scripts/zenodo/xml_to_parquet.py            # records-xml -> parquet-metadata/ (resumable)
python scripts/zenodo/biosyslit_to_parquet.py      # biosyslit JSON -> parquet-biosyslit/
python scripts/zenodo/deleted_to_parquet.py        # both deleted ledgers -> *.parquet
python scripts/zenodo/make_metadata.py             # schema.json + croissant.jsonld (reads row counts)

# quick test slices
python scripts/zenodo/xml_to_parquet.py --max-records 5000 --out /tmp/ztest --fresh
```

- Input:  `data/zenodo/records-xml-<date>.tar.gz`, `biosyslit-records-json-<date>.tar.gz`, `*-deleted-<date>.csv.gz`
- Output: `data/zenodo/parquet-metadata/part-*.parquet` (~500k rows/file), `parquet-biosyslit/`, `*.parquet`
- Resume: checkpointed in `_checkpoint.json` each time a file closes; re-running skips
  finished records and rewrites the partial last file.
- **Do not run the two big conversions concurrently on Windows** — two 20-worker
  pools can crash with WinError 1450 (multiprocessing pipes). Run them
  sequentially (they are resumable either way).

## `parquet-metadata` schema (DataCite-compatible)

One row per Zenodo record. Scalars are typed columns; every nested DataCite
field is a JSON string with the DataCite JSON key names (NULL when empty), so
the `scripts/datacite/` example queries work verbatim. `record_id` is the only
added column.

| Column | Type | Notes |
|---|---|---|
| `doi`, `prefix` | string | primary join key; `prefix` = 10.5281 for Zenodo DOIs |
| `record_id` | string | Zenodo recid — landing page `https://zenodo.org/records/<record_id>` |
| `publisher`, `publication_year`, `published`, `updated` | | `published`/`updated` from DataCite Issued/Updated dates |
| `resource_type_general`, `resource_type`, `title` | string | first title; full list in `titles_json` |
| `language`, `version`, `schema_version`, `url` | string | `schema_version` = 4.5 |
| `*_json` | string (JSON) | `creators_json`, `titles_json`, `subjects_json`, `contributors_json`, `dates_json`, `related_identifiers_json`, `descriptions_json`, `geo_locations_json`, `funding_references_json`, `rights_list_json`, `alternate_identifiers_json`, `sizes_json`, `formats_json`, `types_json` |
| `extra_json` | string (JSON) | leftover fields (e.g. `datacentreSymbol`) |

Columns present in the DataCite Parquet but absent from the XML export (usage
metrics, `client_id`, `state`, `container_json`, …) are simply not emitted —
`UNION BY NAME` fills them with NULL.

## `parquet-biosyslit` schema (Zenodo-native, richer)

The Biodiversity Literature Repository slice keeps what the DataCite XML drops:
file listings, IIIF manifest links, usage stats, and Darwin Core taxonomy.

| Column | Notes |
|---|---|
| `doi`, `record_id`, `parent_id`, `parent_doi` | `parent_doi` = concept DOI shared across versions |
| `created`, `updated`, `publication_date`, `publisher` | |
| `resource_type_id`, `resource_type_title` | e.g. `publication-taxonomictreatment` / "Taxonomic treatment" |
| `title`, `description`, `is_published`, `access_status` | |
| `communities` | JSON array of community slugs (always includes `biosyslit`) |
| `views`, `unique_views`, `downloads`, `unique_downloads` | all-versions stats |
| `file_count`, `total_bytes`, `files_json` | |
| `iiif_manifest` | IIIF Presentation manifest URL |
| `custom_fields_json` | **Darwin Core**: `dwc:kingdom/phylum/class/order/family/genus`, `dwc:taxonRank`, `dwc:scientificNameAuthorship`, `journal:journal` |
| `creators_json`, `subjects_json`, `identifiers_json`, `related_identifiers_json`, `rights_json`, `additional_descriptions_json`, `references_json`, `pids_json`, `extra_json` | Zenodo REST shapes |

## Example DuckDB queries

```sql
-- scan everything
CREATE VIEW zen AS SELECT * FROM 'data/zenodo/parquet-metadata/part-*.parquet';

-- records per resource type
SELECT resource_type_general, count(*) n FROM zen GROUP BY 1 ORDER BY n DESC;

-- creators with ORCIDs (identical to the DataCite query)
SELECT doi, json_extract_string(c.value, '$.name') creator,
       json_extract_string(n.value, '$.nameIdentifier') orcid
FROM zen, json_each(creators_json) c,
     json_each(json_extract(c.value, '$.nameIdentifiers')) n
WHERE json_extract_string(n.value, '$.nameIdentifierScheme') = 'ORCID' LIMIT 10;

-- the version/citation EDGE LIST (precursor to the graph build)
SELECT doi AS src,
       json_extract_string(j.value, '$.relationType')         AS rel,
       json_extract_string(j.value, '$.relatedIdentifier')    AS dst,
       json_extract_string(j.value, '$.relatedIdentifierType') AS dst_type
FROM zen, json_each(related_identifiers_json) j
WHERE related_identifiers_json IS NOT NULL;

-- Zenodo UNION DataCite, joined on doi
SELECT * FROM zen
UNION BY NAME
SELECT * FROM 'data/datacite/parquet-2025/part-*.parquet';

-- biodiversity: treatments per family
SELECT json_extract_string(custom_fields_json, '$."dwc:family"[0]') family, count(*) n
FROM 'data/zenodo/parquet-biosyslit/part-*.parquet'
WHERE custom_fields_json IS NOT NULL GROUP BY 1 ORDER BY n DESC LIMIT 20;

-- exclude deleted records
SELECT count(*) FROM zen
WHERE record_id NOT IN (SELECT record_id FROM 'data/zenodo/records-deleted.parquet');
```

The `related_identifiers_json` edge list is the natural precursor to a `.rete`
graph build; `doi` ties Zenodo to the rest of the scholarly graph.
