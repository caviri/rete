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

## Parquet → RDF: `datacite.rete`

`parquet_to_nt.py` turns the five Parquet sets into N-Triples in the `dcite:`
vocabulary (`datacite.ttl`, `https://w3id.org/rete/datacite#`, OWL 2 QL-safe;
`datacite-ontology.nt` is the same ontology as N-Triples and is prepended to the
stream). `build_rete.sh` is the whole recipe; run it inside `rete-dev` with
pyarrow, duckdb and pyoxigraph on `PYTHONPATH`:

```
bash scripts/datacite/build_rete.sh data/datacite data/datacite/rebuild 14000 10
#   1. parquet_to_nt.py plan   R9/R10 record choice (DuckDB) -> plan.parquet (+ .json stats)
#   2. parquet_to_nt.py emit   N-Triples on stdout, 10 worker processes, every IRI checked
#                              by pyoxigraph, rule tally -> datacite.tally.json
#   3. rete build - --strict --memory-budget-mb 14000 --card-file cards/datacite.card.json
python3 scripts/datacite/make_metadata.py --tally data/datacite/rebuild/datacite.tally.json
#   regenerates schema.json, croissant.jsonld (+ the datacite.rete FileObject) and the
#   Dataset Card the build embeds (rules + counts). The converter is deterministic, so
#   the card is made from a dry `emit` before the build and checked against the build's tally.
python3 -m unittest scripts/tests/test_datacite_parquet_to_nt.py
```

### Principle: faithful to DataCite

The graph says what DataCite says. Literals are written as DataCite has them
(U+FFFD, double-encoded UTF-8, stray whitespace and BOMs are upstream and stay; a
C0 control is written as an N-Triples `\u` escape rather than removed). An
identifier is rewritten only when the rewrite is deterministic and names the same
thing (`https://doi.org/10.x/y` *is* the DOI `10.x/y`). A value that cannot be
made a valid IRI that way is dropped and counted, never guessed. A value the
converter will not mint or type is kept as text (`dcite:rawIdentifier`,
`dcite:publicationYearText`, `dcite:resourceTypeGeneralText`, an untyped
`dcite:landingPage`). Every IRI written is checked by oxiri through pyoxigraph —
the parser `rete build --strict` and Oxigraph use — so validity is decided by a
parser, not by the converter's own escaping rules.

### One record per DOI (R9, R10)

The three metadata snapshots overlap almost completely (2025 holds 99.6 % of the
2024 DOIs). Each DOI is converted once: from the 2025 file, else 2024, else 2023;
within a file, from the row with the latest `updated` (ties: the later file and
row). Every record says where it came from:

```
?r dcite:fromSnapshot dcite:snapshot-2025   # or -2024 / -2023
```

so "only what the 2025 file says" is one triple pattern. A record from 2024 or
2023 is a DOI absent from every later file. The PID Links files are an event
log, not snapshots of one record, so both are converted; an event present in both
collapses into the same `dcite:PidRelation`.

### Rules

| # | Rule |
|---|---|
| R1 | One DOI normaliser for related identifiers, funder ids and link endpoints: trim, lower-case, strip `https?://(dx.\|www.)?doi.org/`, `doi:`, `doi/`, `org/`, a leading `%20` / `%7C%5D`, spaces inside the `10.NNNN/` prefix; a doi.org URL path's `%2F` is the `/` it encodes. Result must match `10.\d{4,9}/\S+`. |
| R2 | Percent-encode what RFC 3987 forbids — `[ ]`, a second `#`, a `%` without two hex digits, controls, U+FFFD, private-use and noncharacters — byte for byte, exactly as `rete export --sanitize-iris` (`crates/rete-core/src/iri.rs`) does, so an IRI that was valid never moves and a SICI DOI joins the sanitized Crossref/OpenAIRE dumps. Then pyoxigraph decides; an IRI it still rejects is dropped. |
| R3 | Funder IRI: ROR (check digits must pass; a bare id typed ROR, or one id inside a malformed ror.org URL) > Crossref Funder ID (any DOI after R1; a Funder Registry number `100…`/`5011…` typed `Crossref Funder ID`, bare or as a doi.org path, gets its `10.13039/` prefix) > `<record>/funder/<sha1(NFC(name))[:16]>`. The name is always `dcite:agentName`, never truncated; an id not minted is `dcite:rawIdentifier` + `dcite:rawIdentifierScheme`. |
| R4 | An empty or `#PLACEHOLDER_PARENT_METADATA_VALUE#` funder name is no name; a reference with neither name nor usable id is dropped. |
| R5 | A link endpoint without a scheme is rescued by R1 or dropped with its event; heuristic-only values (`1007/s10584…`, `0.1109/…`, `journal.pone.0199499`) are dropped. A doi.org URL whose path is not a DOI is kept as the URL it is. |
| R6 | `hdl:x` → `https://hdl.handle.net/x`. `https://orcid.org/` is minted only when the ISO 7064 mod 11-2 check digit passes (16 digits without hyphens are re-hyphenated, a lower-case `x` upper-cased). A creator's failing ORCID becomes `dcite:rawIdentifier` on a record-local agent; a failing ORCID *URL* in the PID Links is kept verbatim (it is the source's IRI, not one we mint). |
| R7 | All 38 DataCite 4.6 `relationType` values have their own property, for metadata and links alike (the 2026-07 build had 14 and dropped the rest from metadata, folding them into `isRelatedTo` in the links). PID-Links-only types: `is-authored-by` → `dcite:isAuthoredBy`, `is-authored-at` → `dcite:isAuthoredAt`, `is-affiliated-with` → `dcite:isAffiliatedWith`, `is-funded-by` → `dcite:fundedBy`; any other (`is-review-of`, `has-preprint`, …) is `dcite:isRelatedTo`. Every `dcite:PidRelation` carries its exact type as a `dcite:rel-*` concept. |
| R8 | A DOI-typed related identifier with no DOI after R1 is dropped. A DOI inside other text is taken only when exactly one stands there, not glued to letters or digits on either side (`10.5281/zenodo.1 (DOI)` yes; `10.510.5281/…`, `10.3969/j.issn.1000 1565…` no). |
| R9 | One record per DOI, newest snapshot wins; `dcite:fromSnapshot`. |
| R10 | Within a snapshot the latest `updated` wins. |
| R11 | `occurredAt` `0000-01-01…` (DataCite's "unknown") is not emitted. A `publicationYear` outside 1000–2027 is `dcite:publicationYearText`, not `xsd:gYear`. |
| R12 | `landingPage` is `xsd:anyURI` only if it parses as an IRI, else a plain string. |
| R13 | Literals are not repaired. |
| R14 | `rorId` only when the ROR check digits pass (written as `https://ror.org/<id>`); otherwise `dcite:rawIdentifier`. |

### Counts (2026-10 build)

From the converter's tally over all five sets (occurrences; the Dataset Card of
`datacite.rete` carries the same table, and `datacite.tally.json` beside the file
has every counter per set with examples). 109,263,993 records (2025: 108,468,903;
2024 only: 308,793; 2023 only: 486,297) and 760,802,549 PID Links events became
11,731,454,390 statements, every one parsed by Oxigraph 0.5.11 before the build.

| Rule | Rescued | Encoded | Kept as text | Dropped |
|---|---:|---:|---:|---:|
| R1 | 173,978,334 | 0 | 83 | 0 |
| R2 | 0 | 302,503 | 0 | 0 |
| R3 | 13 | 0 | 200,193 | 0 |
| R4 | 0 | 0 | 0 | 20 |
| R5 | 84 | 0 | 0 | 460 |
| R6 | 14,666 | 0 | 14,944 | 0 |
| R7 | 28,884,493 | 0 | 0 | 0 |
| R8 | 0 | 0 | 0 | 1,204,136 |
| R9 | 0 | 0 | 0 | 124,087,770 |
| R10 | 0 | 0 | 0 | 731 |
| R11 | 0 | 0 | 271,014 | 27,894 |
| R12 | 0 | 0 | 10,624 | 0 |
| R13 | 0 | 0 | 0 | 0 |
| R14 | 51,705 | 0 | 8,438 | 0 |

R1's rescues are mostly `https://doi.org/…` related identifiers (88.2 M, which the
2026-07 build wrote as `https://doi.org/https://doi.org/…`) and doi.org-URL funder
ids (84.8 M; the old build stripped `https://doi.org/` but turned
`http://dx.doi.org/10.13039/…` into `https://doi.org/http://dx.doi.org/…`). R7's are edges using a property this build
added (e.g. 14.3 M `hasMetadata` link events that were `isRelatedTo`). R5 drops
460 link events (81 of them the audit's scheme-less values, 377 values that start
like a DOI but are not one); R9/R10 "dropped" are superseded **rows**, not DOIs.

Not converted: related identifiers that are not DOI-typed (URL, Handle, ISSN,
arXiv, PMID, …), descriptions, typed dates, rights, contributors other than
creators, geo locations, sizes and formats.
