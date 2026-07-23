# Crossref Public Data File → Parquet

Converts the [Crossref Public Data File](https://www.crossref.org/learning/public-data-file/)
(March 2026 release: 35,908 numbered `*.jsonl.gz` files, 207.7 GiB compressed /
~1.1 TiB of JSON, **179,536,204 DOI records**, CC-BY 4.0) into two joinable
Parquet tables **without ever extracting the archive** — each worker owns a
contiguous chunk of the source gz files end-to-end and writes its own Parquet
parts directly.

Unlike the DataCite/ORCID/OpenAIRE dumps (one giant tar the main thread streams
member-by-member), the Crossref dump is thousands of *independent* gzip files,
so there is no central bottleneck, no rolling writer, and no checkpoint file: a
task is done iff both of its final part files exist, which makes resume =
"skip tasks whose outputs already exist". Stale `*.tmp` from a killed run are
swept on startup.

Results (row counts, 18-worker run, ~30 min wall clock; zero bad lines):

| Table | Rows | Output | Notes |
|---|---|---|---|
| `works` | 179,536,204 | `parquet-2026/works/`, 499 files, 41.6 GB | one row per DOI |
| `refs`  | 2,742,943,747 | `parquet-2026/refs/`, 499 files, 127.8 GB | citation edge list; 1,986,466,765 (72.4%) are DOI→DOI |

Full-scan DuckDB counts over `works` are sub-second (Parquet metadata);
aggregations over the 2.74B-edge `refs` table run in seconds to low minutes.

```
python scripts/crossref/jsonl_to_parquet.py                    # full run (defaults, resumable)
python scripts/crossref/jsonl_to_parquet.py --max-tasks 2 --files-per-task 3 --out C:/tmp/t --fresh
```

- Input:  `data/crossref/March 2026 Public Data File from Crossref/<N>.jsonl.gz`
  (5,000 DOI-sorted records each; override with `--src`)
- Output: `data/crossref/parquet-2026/{works,refs}/part-*.parquet`
  (one `part-<task>.parquet` per table per task; ~360k works / task)
- Resume: re-running the same command skips completed tasks and finishes the
  rest. Disk guard: below `--min-free-gib` (default 25) workers stop cleanly
  and the run stays resumable instead of filling the drive.

## `works` schema

One row per DOI. Scalars are typed columns; every nested Crossref field is kept
whole as a JSON string (NULL when empty/absent) so nothing is lost. `DOI` is
lower-cased. Dropped as derivable/constant — each re-added to `extra_json` only
if it ever deviates: `URL` (= `https://doi.org/<doi>`), `source`
(= `"Crossref"`), `score` (search-only, constant in the dump), `reference-count`
(deprecated alias of `references-count`). The `reference` array is **not** here —
it becomes the `refs` table.

| Column | Type | Notes |
|---|---|---|
| `doi`, `prefix`, `member` | string | `member` = Crossref member (publisher) id |
| `type` | string | `journal-article`, `book-chapter`, `dataset`, … |
| `title`, `subtitle`, `original_title` | string | first element of Crossref's 1-element arrays; extras → `extra_json.<field>_rest` |
| `container_title`, `short_container_title` | string | journal/book title (first element) |
| `publisher`, `publisher_location` | string | |
| `volume`, `issue`, `page`, `article_number` | string | |
| `language` | string | |
| `issn`, `isbn` | string | first identifier (convenience); full lists in `issn_json` / `isbn_json` |
| `issued`, `published`, `published_print`, `published_online`, `accepted` | string | `date-parts` flattened to `YYYY[-MM[-DD]]` (partial dates kept partial) |
| `issued_year` | int32 | convenience year of `issued` |
| `created`, `deposited`, `indexed` | string | full ISO `date-time` |
| `is_referenced_by_count`, `references_count` | int32 | inbound citation count / outbound reference count |
| `abstract` | string | JATS XML string (present for ~22%) |
| `resource_url` | string | `resource.primary.URL` (publisher landing page); other `resource` keys → `extra_json` |
| `update_policy` | string | |
| `author_json`, `editor_json`, `translator_json`, `chair_json` | string (JSON) | contributor arrays — each entry has `given`/`family`/`sequence`/`affiliation`/`ORCID` |
| `license_json`, `link_json`, `funder_json`, `assertion_json`, `relation_json` | string (JSON) | |
| `update_to_json`, `updated_by_json`, `alternative_id_json`, `archive_json` | string (JSON) | |
| `event_json`, `institution_json`, `journal_issue_json`, `content_domain_json` | string (JSON) | |
| `clinical_trial_number_json`, `aliases_json`, `free_to_read_json`, `review_json`, `standards_body_json`, `subject_json` | string (JSON) | |
| `extra_json` | string (JSON) | any attribute not covered above, plus `*_raw` for values that overflowed a typed column |

## `refs` schema

One row per reference entry across all works — the citation **edge list**
(Crossref's counterpart to DataCite PID Links). `doi -> ref_doi` is a citation
whenever the reference is DOI-matched; otherwise only the raw citation string is
available.

| Column | Type | Notes |
|---|---|---|
| `doi` | string | the **citing** work (join key to `works.doi`) |
| `ref_index` | int32 | position of the reference within the work |
| `key` | string | Crossref's per-reference key |
| `ref_doi` | string | the **cited** DOI, lower-cased — NULL when the reference is unstructured/not DOI-matched |
| `doi_asserted_by` | string | `crossref` (matched by Crossref) or `publisher` (deposited) |
| `year` | int32 | cited work's year, **normalised to a real 1000–2030 value or NULL**; salvaged from messy originals (`2020a` → 2020, `20200101` → 2020, `2019-2020` → 2019); unrecoverable originals (the `0` sentinel, `n.d.`, `in press`, …) → NULL with the raw kept in `rest_json.year_raw` |
| `unstructured` | string | raw citation text when no structured metadata was deposited |
| `rest_json` | string (JSON) | all other reference fields (`author`, `volume`, `first-page`, `journal-title`, `article-title`, `ISSN`, `issue`, …) + `year_raw` |

## Data-quality note

Crossref reference `year` values are messy in two ways that a naive parse gets
wrong:

* **overflow** — `2020a` / `2019b` author-year disambiguators, `YYYYMMDD`
  timestamps, and date ranges are not plain ints; the raw big-int forms even
  overflow int32 (`OverflowError: Python int too large to convert to C long`,
  since a C long is 32-bit on Windows);
* **in-range junk** — `~4.3M` references carry `year = 0` (a "no year"
  sentinel), plus `YYYYMMDD` stamps like `20200101` that *fit* int32 and would
  otherwise be stored verbatim as a 20-million "year".

`salvage_year()` normalises `year` to the invariant **a real 1000–2030 year or
NULL**: it takes the first plausible year in the value's digits (`2020a` → 2020,
`20200101` → 2020, `2019-2020` → 2019) and NULLs genuine junk (`0`, `..`,
`n.d.`, `in press`). Every original that can't be salvaged is preserved as a
string in `refs.rest_json.year_raw` (or `works.extra_json.<field>_raw` for the
work-level count columns) — nothing is dropped. After the build, all 2.74B
`refs` rows satisfy the invariant: `year` is NULL or in 1000–2030 (median 2006),
with ~4.53M unrecoverable originals kept in `year_raw`. Row counts match
Crossref's published record count exactly.

Because the converter is resumable, this normalisation lives in the parser and
runs on the **first** pass — parts written by an earlier version of the code are
frozen and can only be corrected by a rebuild from source (which is what the
intact `*.jsonl.gz` are for).

## Example DuckDB queries

```sql
-- register the tables
CREATE VIEW works AS SELECT * FROM 'data/crossref/parquet-2026/works/part-*.parquet';
CREATE VIEW refs  AS SELECT * FROM 'data/crossref/parquet-2026/refs/part-*.parquet';

-- works per type per year
SELECT type, issued_year, count(*) n
FROM works WHERE issued_year BETWEEN 2000 AND 2026
GROUP BY 1, 2 ORDER BY n DESC LIMIT 20;

-- most-cited works (Crossref's own inbound count)
SELECT doi, title, is_referenced_by_count
FROM works ORDER BY is_referenced_by_count DESC NULLS LAST LIMIT 20;

-- the DOI→DOI citation graph: who cites 10.1038/nature12373
SELECT doi AS citing FROM refs WHERE ref_doi = '10.1038/nature12373' LIMIT 50;

-- outdegree distribution (references per work, DOI-matched only)
SELECT n_refs, count(*) works FROM (
  SELECT doi, count(ref_doi) n_refs FROM refs GROUP BY doi
) GROUP BY 1 ORDER BY 1 LIMIT 30;

-- unnest authors with ORCIDs
SELECT doi,
       json_extract_string(a.value, '$.family') AS family,
       json_extract_string(a.value, '$.ORCID')  AS orcid
FROM works, json_each(author_json) a
WHERE author_json IS NOT NULL
  AND json_extract_string(a.value, '$.ORCID') IS NOT NULL
LIMIT 20;

-- funders (join key: funder DOI / name)
SELECT json_extract_string(f.value, '$.name') AS funder, count(*) n
FROM works, json_each(funder_json) f
WHERE funder_json IS NOT NULL
GROUP BY 1 ORDER BY n DESC LIMIT 20;
```

## Joining the scholarly graph

`works.doi` and `refs.ref_doi` are lower-cased DOIs, the same key used across
the sibling datasets, so Crossref stitches into DataCite / OpenAIRE / ORCID /
DBLP / OpenCitations without translation:

```sql
-- Crossref citing works that reference a DataCite dataset DOI
SELECT m.resource_type_general, count(*) n
FROM 'data/crossref/parquet-2026/refs/part-*.parquet' r
JOIN 'data/datacite/parquet-2025/part-*.parquet' m ON r.ref_doi = m.doi
WHERE m.resource_type_general = 'Dataset'
GROUP BY 1 ORDER BY n DESC;

-- reconcile a Crossref author ORCID against the ORCID person table
SELECT p.orcid, p.given_names, p.family_name, count(*) crossref_works
FROM works w, json_each(w.author_json) a
JOIN 'data/orcid/parquet-summaries/person/part-*.parquet' p
  ON p.orcid = regexp_extract(json_extract_string(a.value,'$.ORCID'), '(\d{4}-\d{4}-\d{4}-[\dxX]{4})')
GROUP BY 1,2,3 ORDER BY crossref_works DESC LIMIT 20;
```

## Metadata artifacts

`make_metadata.py` regenerates two machine-readable descriptions of the Parquet
(edit the column lists / `DESC` there, never the outputs by hand):

- `data/crossref/schema.json` — JSON Schema (draft 2020-12) with two `$defs`,
  `work` (58 cols) and `reference` (8 cols); every column typed and described,
  `_json` columns flagged `contentMediaType: application/json`.
- `data/crossref/croissant.jsonld` — MLCommons Croissant 1.0: 2 FileSets +
  2 RecordSets, with `refs.doi` and `refs.ref_doi` declared as `references` to
  `works.doi` (the citation-graph join).

```
python scripts/crossref/make_metadata.py
```

The RDF/OWL domain ontology is hand-authored:

- `data/crossref/crossref.ttl` — the **Crossref ontology** (`cx:` =
  `https://w3id.org/rete/crossref#`): `cx:Work` (+ per-type subclasses),
  `cx:Reference` (reified citation edge), `cx:Agent`, `cx:Funder`/`cx:Funding`,
  and the `cx:cites` shortcut. Reuses FaBiO / CiTO / PRISM / FRAPO / PRO /
  schema.org / Dublin Core and stays OWL 2 QL-safe (for rete's lazy reasoner).
  It is wired into the scholarly graph by `data/scholar/scholar.ttl`
  (`cx:Work ⊑ scholar:Work`, `cx:doi`/`cx:citedDOI ⊑ scholar:doi`,
  `cx:orcid ⊑ scholar:orcid`, `cx:isbn ⊑ scholar:isbn`,
  `cx:funderId ⊑ scholar:fundref` — Crossref is the Funder-ID authority), and
  `cx:cites ⊑ cito:cites` unifies its citation edges with DataCite's.
