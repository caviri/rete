# GoTriple metadata dataset → Parquet

Converts the [GoTriple metadata dataset](https://zenodo.org/records/18185971)
(Zenodo 18185971, **CC0**) — metadata for **6,074,813 Social-Sciences & Humanities
publications** with a full-text link, from the EU-funded GoTriple discovery
platform (gotriple.eu / TRIPLE / OPERAS) — into queryable Parquet.

The source ships one gzipped JSON-Lines file per **discipline**; each line is a
schema.org-flavoured JSON document. `jsonl_to_parquet.py` streams each file
(bounded memory) into `parquet/<discipline>.parquet` — glob-scannable, one row
per publication, with a `discipline` column so a full-corpus glob also works.

## Results

| Output | Rows | Files |
|---|---|---|
| `data/go-triple/parquet/*.parquet` | 6,074,813 | 26 (one per discipline) |

Biggest: History 541,609 · Sociology 493,039 · Environmental studies 439,156 ·
Linguistics 418,198 · Education 368,025. (The README lists a 27th discipline,
Political Science `scipo`, but no `scipo` file shipped.)

```
python scripts/gotriple/jsonl_to_parquet.py            # full -> parquet/
python scripts/gotriple/jsonl_to_parquet.py --limit 5000   # test slice
python scripts/gotriple/make_metadata.py               # schema.json + croissant.jsonld
```

- Input:  `data/go-triple/*_merged.jsonl.gz` (downloaded from Zenodo, MD5-verified)
- Output: `data/go-triple/parquet/<discipline>.parquet` (zstd), + `schema.json`,
  `croissant.jsonld`, and the `go-triple.ttl` ontology.

## Schema

One row per publication. Analytically useful scalars are typed columns; every
nested GoTriple field is kept whole as a JSON string (NULL when empty).

| Column | Notes |
|---|---|
| `id` | native GoTriple id (primary key) |
| `doi` | first non-empty DOI — the cross-dataset **join key** (~69% coverage) |
| `title` | first headline text (full multilingual list in `headline_json`) |
| `discipline` | the file's discipline label (hist, socio, droit, …) |
| `primary_topic` | highest-confidence topic (scored list in `topic_json`) |
| `date_published`, `datestamp`, `language`, `provider`, `publisher`, `url` | first/scalar |
| `is_cluster`, `is_duplicate`, `cluster_children_count`, `n_authors` | dedup + convenience |
| `*_json` | `abstract_json` (multilingual + machine translations), `author_json`, `keywords_json`, `knows_about_json` (**linked SSH-LCSH subject authorities** — semantics.gr URIs + multilingual labels), `topic_json`, `provider_json`, `identifier_json`, `spatial_coverage_json`, … |
| `extra_json` | any field not promoted to a column |

## Ontology & federation

`data/go-triple/go-triple.ttl` (`https://w3id.org/rete/gotriple#`, prefix `gtr:`)
models a `gtr:Document` as a `scholar:Work`, with `gtr:doi ⊑ scholar:doi`. Under
the [scholar-alignment](../../data/scholar/scholar.ttl) canonical-IRI policy,
GoTriple works mint at `https://doi.org/{doi}` and **auto-merge** with the
DataCite / Zenodo / OpenAIRE / OpenCitations graphs on the shared DOI — GoTriple
brings the **SSH** coverage those STEM-heavy sources lack. It also carries the
GoTriple discipline SKOS scheme and links to SSH-LCSH subject authorities.

## Example DuckDB queries

```sql
CREATE VIEW gt AS SELECT * FROM 'data/go-triple/parquet/*.parquet';

-- publications per discipline
SELECT discipline, count(*) n FROM gt GROUP BY 1 ORDER BY n DESC;

-- DOI coverage
SELECT round(100.0*count(doi)/count(*),1) pct_with_doi FROM gt;

-- most common linked subjects (SSH-LCSH), English label
SELECT json_extract_string(l.value,'$.text') subject, count(*) n
FROM gt, json_each(knows_about_json) k, json_each(json_extract(k.value,'$.labels')) l
WHERE knows_about_json IS NOT NULL
  AND json_extract_string(l.value,'$.lang') = 'en'
GROUP BY 1 ORDER BY n DESC LIMIT 20;

-- join to DataCite/Zenodo on DOI (SSH works that are also registered there)
SELECT count(*) FROM gt g
JOIN 'data/datacite/parquet-2025/part-*.parquet' d ON lower(g.doi) = lower(d.doi);
```
