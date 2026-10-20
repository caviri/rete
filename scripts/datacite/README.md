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
bash scripts/datacite/build_rete.sh data/datacite data/datacite/rebuild2 14000 10
#   1. parquet_to_nt.py plan   R9/R10 record choice and R18 event choice (DuckDB) ->
#                              plan.parquet (+ .links.parquet, .json stats). Refuses to run
#                              on an identity disagreement, an IRI collision or a bad uuid.
#   2. parquet_to_nt.py emit   N-Triples on stdout, 10 worker processes, every IRI checked
#                              by pyoxigraph, rule tally -> datacite.tally.json, every
#                              record IRI -> OUT/guard/ (checked for duplicates)
#   3. rete build - --strict --memory-budget-mb 14000 --card-file cards/datacite.card.json
#                              -> datacite.rete.part
#   4. guard.py                records/events reconcile with the plan, no single-valued
#                              property twice, embedded card == this tally's card; only
#                              then datacite.rete.part -> datacite.rete
CARD_ONLY=1 bash scripts/datacite/build_rete.sh data/datacite data/datacite/rebuild2 - 12
#   steps 1-2 without rete (36 min), then make_metadata.py --tally rewrites
#   cards/datacite.card.json (+ schema.json, croissant.jsonld). Run it and commit the
#   card whenever the converter or the Parquet changes; the converter is deterministic,
#   so step 4 of the real build proves the card describes the file.
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
log, not snapshots of one record, so both are converted; an event is its `uuid`
(R18), and one present in both files is taken once, from May 2025.

### One identity, checked (R0 and the guards)

A DOI is the string DataCite registered, with ASCII whitespace trimmed and
lower-cased — nothing more. `10.18454/\u00a0pmi.2016.5.712` and
`10.18454/pmi.2016.5.712` are two registered DOIs (both resolve), and
`10.13147/soe.2018.013\u00a0` resolves while `10.13147/soe.2018.013` does not, so
neither an NBSP nor any other rescue is ever applied to a record's own DOI. The
plan keys records with the same function (`doi_key` in Python, `PLAN_KEY_SQL` in
DuckDB) and refuses to run if the two disagree on any DOI, or if two registered DOIs
would mint the same subject IRI. Registered DOIs that the R1 rescues would rewrite
are listed by the plan and passed to `emit`, so a related identifier or link that
names one of them lands on its record.

`build_rete.sh` then refuses the build (it leaves `datacite.rete.part`) unless
`guard.py` finds: no record stating a single-valued property twice, no record IRI
emitted twice (every record IRI is written to `OUT/guard/` and counted by DuckDB),
records per snapshot equal to the plan, one `dcite:PidRelation` per kept event, and
an embedded Dataset Card identical to the card this build's tally produces. The
hunts behind these rules are re-runnable scripts in `dev/datacite-defect-hunt/`.

### Rules

| # | Rule |
|---|---|
| R0 | One identity per DOI: the registered string, ASCII whitespace trimmed, lower-cased. Plan, record IRI and pointers share it; a record's own DOI never goes through R1. The plan refuses Python/DuckDB key disagreements and IRI collisions between registered DOIs. |
| R1 | One DOI normaliser for related identifiers, funder ids and link endpoints (a value that is a registered DOI is that DOI, untouched): trim, lower-case, strip `https?://(dx.\|www.)?doi.org/`, `doi:`, `doi/`, `org/`, a leading `%20` / `%7C%5D`, spaces inside the `10.NNNN/` prefix; a doi.org URL path's `%2F` is the `/` it encodes. Result must match `10.\d{4,9}/\S+`. |
| R2 | Percent-encode what RFC 3987 forbids — `[ ]`, a second `#`, a `%` without two hex digits, controls, U+FFFD, private-use and noncharacters — byte for byte, exactly as `rete export --sanitize-iris` (`crates/rete-core/src/iri.rs`) does, so an IRI that was valid never moves and a SICI DOI joins the sanitized Crossref/OpenAIRE dumps. Then pyoxigraph decides; an IRI it still rejects is dropped. |
| R3 | Funder IRI: ROR (check digits must pass; a bare id typed ROR, or one id inside a malformed ror.org URL) > Crossref Funder ID (any DOI after R1; a Funder Registry number `100…`/`5011…` typed `Crossref Funder ID`, bare or as a doi.org path, gets its `10.13039/` prefix) > `<record>/funder/<sha1(NFC(name))[:16]>`. The name (never truncated) and an id not minted (`dcite:rawIdentifier` + `dcite:rawIdentifierScheme`) are on the reference (R16). |
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
| R15 | Under `10.13039/` (the Funder Registry) a DOI is `10.13039/<digits>`. A value naming exactly one Funder Registry number (`10.13039/http://dx.doi.org/10.13039/501100001809`, `10.13039/501100002569"`) is that Funder ID; a placeholder (`10.13039/#`, `funder%20id`, `xx`, a number glued to text) is not minted: `dcite:rawIdentifier` in metadata, event dropped in the links. |
| R16 | Each fundingReference is a record-scoped `dcite:FundingReference` (`<doi>/funding/<n>`, `dcite:hasFundingReference`) with `dcite:funder`, `dcite:funderName` and any id not minted. The shared funder IRI carries no name: before, DFG's Funder ID was also named "BMBF" and "European Commission" by other records. `dcite:fundedBy` stays as the shortcut. |
| R18 | A PID Links event is its `uuid`: one `dcite:PidRelation` `<{subject}/event/{uuid}>` with exactly one subject, object, type, source and time. Before, events with the same subject, type and object shared one node: 3.66 M nodes held 8.83 M events, 6.38 M nodes had two `occurredAt` and 0.76 M two `assertedBy`, unpaired. |
| R19 | A creator's name (`dcite:creatorName`), affiliations, ROR and ids not minted are on the record's `dcite:AgentRole`; an `https://orcid.org/` agent carries only `dcite:orcid` (one ORCID collected 2,442 names from 2025 records alone). |

### Counts (2026-10-20 build)

From the converter's tally over all five sets (occurrences; the Dataset Card of
`datacite.rete` carries the same table, and `datacite.tally.json` beside the file
has every counter per set with examples). 109,263,993 records (2025: 108,468,903;
2024 only: 308,793; 2023 only: 486,297) and 613,426,534 PID Links events (the
147,376,015 events of the 2023 file that are also in May 2025 are taken once) became
10,961,620,937 statements, every IRI parsed by Oxigraph 0.5.11 before the build.

| Rule | Rescued | Encoded | Kept as text | Dropped |
|---|---:|---:|---:|---:|
| R0 | 0 | 0 | 6 | 0 |
| R1 | 173,978,232 | 0 | 42 | 0 |
| R2 | 0 | 212,554 | 0 | 0 |
| R3 | 13 | 0 | 200,193 | 0 |
| R4 | 0 | 0 | 0 | 20 |
| R5 | 48 | 0 | 0 | 247 |
| R6 | 14,666 | 0 | 14,544 | 0 |
| R7 | 20,048,535 | 0 | 0 | 0 |
| R8 | 0 | 0 | 0 | 1,204,136 |
| R9 | 0 | 0 | 0 | 124,087,770 |
| R10 | 0 | 0 | 0 | 731 |
| R11 | 0 | 0 | 271,014 | 13,947 |
| R12 | 0 | 0 | 10,624 | 0 |
| R13 | 0 | 0 | 0 | 0 |
| R14 | 51,705 | 0 | 8,438 | 0 |
| R15 | 431 | 0 | 11 | 129 |
| R17 | 0 | 0 | 393,842 | 0 |

R16: 87,261,365 `dcite:FundingReference`s. R18: 613,426,158 `dcite:PidRelation`s
(376 events dropped with an endpoint). R19: 107,568,498 creator roles held by an
orcid.org agent, 275,686,388 by a record-local one.

R1's rescues are mostly `https://doi.org/…` related identifiers and doi.org-URL
funder ids (84.8 M). R7's are edges using a property the 2026-10 builds added
(e.g. `hasMetadata` link events that were `isRelatedTo` in 2026-07). R9/R10
"dropped" are superseded **rows**, not DOIs. The R2, R5, R7 and R11 counts are
lower than in the #306 build because a 2023 event that May 2025 repeats is now
converted once.

The build stored **10,184,066,895 unique triples** (1,959,467,073 terms, 63.1 GB);
the #306 build had 9,530,064,339 (49.1 GB). Every guard held: record IRIs = DOIs =
109,263,993, no single-valued property stated twice (checked in SPARQL as
`COUNT(*) = COUNT(DISTINCT ?s)` for 32 of them), one PidRelation per event.

Known residuals, kept faithful and counted: DOIs written with percent-escapes
(R17: 248,592 link-object occurrences, 114,526 of them double-encoded — 15,704
distinct values such as `10.1016/s0020-7519%252899%252900141-1`) do not join their unescaped form; a
failing-checksum ORCID URL in the links is kept verbatim, so `http://` and
`https://orcid.org/0000-0000-0000-0000` are two IRIs (19 such ids).

Not converted: related identifiers that are not DOI-typed (URL, Handle, ISSN,
arXiv, PMID, …), descriptions, typed dates, rights, contributors other than
creators, geo locations, sizes and formats.
