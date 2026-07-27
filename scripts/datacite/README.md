# DataCite Public Data File → Parquet

Converts the [DataCite Public Data File](https://datafiles.datacite.org/) tar.gz
(2023: 22 GiB compressed / 197 GiB of JSONL, 52,863,283 DOI records, CC0)
into queryable Parquet **without ever extracting the archive** — members are
streamed one at a time from the compressed tar, parsed by a process pool, and
written as rolling zstd Parquet files.

Results (row counts are exact matches with the published counts; zero bad
lines; 32-core machine, ~18 min wall clock each):

| Dataset | Rows | Output | Source |
|---|---|---|---|
| Public 2023 | 52,863,283 | `parquet-2023/`, 101 files, 15.2 GB | 22 GiB tar.gz / 197 GiB JSON |
| Public 2024 | 72,019,577 | `parquet-2024/`, 131 files, 15.5 GB | 23 GiB tar / 347 GiB JSON |
| Public 2025 | 108,468,906 | `parquet-2025/`, 197 files, 20.4 GB | 32 GiB tar / 615 GiB JSON (30× smaller) |
| PID Links 2023 | 167,844,248 | `parquet-links-2023/`, 34 files, 6.5 GB | 10 GiB tar.gz |
| PID Links May 2025 | 592,958,301 | `parquet-links-may2025/`, 119 files, 20.2 GB | 33 GiB tar.gz |

Full-scan DuckDB aggregations: ~0.2–0.5 s per metadata year, ~10 s over all
593M edges. Do not run two 20-worker conversions concurrently on Windows —
multiprocessing pipes can die with WinError 1450 (it's resumable, but still).

```
python scripts/datacite/tar_to_parquet.py            # 2023 file (defaults, resumable)
python scripts/datacite/tar_to_parquet.py --tar data/datacite/DataCite_Public_Data_File_2025.tar --out data/datacite/parquet-2025
python scripts/datacite/tar_to_parquet.py --max-members 40 --out /tmp/test --fresh
```

- Input:  `data/datacite/DataCite_Public_Data_File_<year>.tar[.gz]`
- Output: `data/datacite/parquet-<year>/part-*.parquet` (~500k rows per file)
- Resume: checkpointed in `_checkpoint.json` every time a file closes; re-running
  the same command skips completed members and rewrites the partial last file.
- Handles both the 2023 layout (`./<prefix>/part_NNNNN.jsonl`, flat records) and
  the 2024+ layout (`dois/updated_YYYY-MM/part_NNNN.jsonl.gz`, REST envelope
  with `attributes`/`relationships`; `client_id` is populated, and the 2024+
  publisher *object* is flattened to its name with the original kept in
  `extra_json.publisher_obj`).

## Schema

One row per DOI. Scalars are typed columns; every nested DataCite field is
kept whole as a JSON string (NULL when empty/absent) so nothing is lost.
Only `suffix` is dropped (`doi = prefix + "/" + suffix`).

| Column | Type | Notes |
|---|---|---|
| `doi`, `prefix` | string | |
| `state` | string | findable (the public file only contains findable) |
| `source`, `is_active`, `reason` | string/bool/string | |
| `client_id` | string | repository id — only in 2024+ files, NULL in 2023 |
| `created`, `registered`, `updated`, `published` | string | ISO timestamps |
| `publication_year` | int32 | non-integer originals moved to `extra_json.publicationYear_raw` |
| `language`, `version`, `metadata_version`, `schema_version`, `url` | | |
| `publisher` | string | |
| `resource_type_general`, `resource_type` | string | from `types` |
| `title` | string | first entry of `titles` (convenience) |
| `citation_count`, `reference_count`, `view_count`, `download_count`, `version_count`, `version_of_count`, `part_count`, `part_of_count` | int32 | usage/citation metrics — 2024+ files only, NULL in 2023 |
| `citations_over_time_json`, `views_over_time_json`, `downloads_over_time_json` | string (JSON) | metric time series — 2024+ files only |
| `types_json`, `container_json`, `creators_json`, `titles_json`, `subjects_json`, `contributors_json`, `dates_json`, `related_identifiers_json`, `related_items_json`, `descriptions_json`, `geo_locations_json`, `funding_references_json`, `rights_list_json`, `identifiers_json`, `alternate_identifiers_json`, `sizes_json`, `formats_json`, `content_url_json` | string (JSON) | full nested fields, NULL when empty |
| `extra_json` | string (JSON) | any attribute key not covered above |

## Example DuckDB queries

```sql
-- scan everything (glob); union both years with union_by_name if needed
CREATE VIEW dois AS SELECT * FROM 'data/datacite/parquet-2023/part-*.parquet';

-- datasets per publisher per year
SELECT publisher, publication_year, count(*) n
FROM dois WHERE resource_type_general = 'Dataset'
GROUP BY 1, 2 ORDER BY n DESC LIMIT 20;

-- unnest a JSON column: relation types across the corpus
SELECT json_extract_string(j.value, '$.relationType') rel, count(*) n
FROM dois, json_each(related_identifiers_json) j
WHERE related_identifiers_json IS NOT NULL
GROUP BY 1 ORDER BY n DESC;

-- creators with ORCIDs
SELECT doi, json_extract_string(c.value, '$.name') creator,
       json_extract_string(n.value, '$.nameIdentifier') orcid
FROM dois, json_each(creators_json) c,
     json_each(json_extract(c.value, '$.nameIdentifiers')) n
WHERE json_extract_string(n.value, '$.nameIdentifierScheme') = 'ORCID'
LIMIT 10;

-- extract the citation/relation EDGE LIST (precursor to the graph build)
SELECT doi AS src,
       json_extract_string(j.value, '$.relationType')       AS rel,
       json_extract_string(j.value, '$.relatedIdentifier')   AS dst,
       json_extract_string(j.value, '$.relatedIdentifierType') AS dst_type
FROM dois, json_each(related_identifiers_json) j
WHERE related_identifiers_json IS NOT NULL;
```

## PID Links files → edge tables

`links_to_parquet.py` converts the PID Links Data Files (DataCite's PID
Graph / Event Data: 2023 = 167.8M events, May 2025 = 593M events) into
Parquet edge tables at `data/datacite/parquet-links-<version>/`:

| Column | Notes |
|---|---|
| `subj_id`, `obj_id` | bare DOI when a doi.org URL, full URL otherwise |
| `relation_type` | `references`, `cites`, `is-identical-to`, `is-supplement-to`, … |
| `source_id` | provenance: `crossref` (literature→data citations), `datacite-related`, … |
| `citation_type` | schema.org type pair, e.g. `ScholarlyArticle-Dataset` |
| `subj_type`, `obj_type` | schema.org `@type` of each endpoint |
| `subj_published`, `obj_published`, `subj_year`, `obj_year` | endpoint publication dates + extracted years |
| `occurred_at`, `created_at`, `updated_at`, `uuid` | event timestamps/id (`occurred_at` of `0000-01-01…` = unknown) |
| `subj_extra_json`, `obj_extra_json`, `extra_json` | endpoint fields beyond id/@type/date_published; unknown top-level keys |

The `prefix` and `doi` pair-arrays are dropped (derivable from subj/obj ids).

```sql
-- who cites DataCite datasets, by year of the citing work
SELECT subj_year, count(*) n
FROM 'data/datacite/parquet-links-2023/part-*.parquet'
WHERE source_id = 'crossref' AND relation_type = 'references'
GROUP BY 1 ORDER BY 1;

-- join edges with DOI metadata
SELECT m.resource_type_general, count(*) n
FROM 'data/datacite/parquet-links-2023/part-*.parquet' l
JOIN 'data/datacite/parquet-2023/part-*.parquet' m ON l.obj_id = m.doi
GROUP BY 1 ORDER BY n DESC;
```
