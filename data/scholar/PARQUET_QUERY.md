# Querying the scholar Parquet on R2 with DuckDB (SQL)

Every scholar dataset's **Parquet companions** are published on Cloudflare R2 and
are directly queryable with SQL — no download, only the byte ranges each query
touches are fetched. This is the *tabular* twin of the `.rete` graphs: same data,
in columnar SQL, ideal for aggregates, joins and crosswalks.

- **Base URL (public HTTP):** `https://data.graphplaza.com/<prefix>/…`
- **Same bytes over the S3 API:** bucket `rete`, i.e. `s3://rete/<prefix>/…`
- Each dataset folder has a `_parquet_manifest.json` listing every file + size.

> Engine: **DuckDB** (tested with 1.5.4). `INSTALL httpfs; LOAD httpfs;` once —
> it gives DuckDB HTTP-range and S3 reads.

---

## The one gotcha: globbing needs the S3 API

Plain `https://` has **no directory listing**, so a wildcard over the public HTTP
endpoint fails:

```sql
-- ❌ Invalid Input Error: Globs (`*`) for generic HTTP file are not supported
SELECT count(*) FROM read_parquet('https://data.graphplaza.com/orcid/parquet-summaries/person/*.parquet');
```

You have **three** ways to read a multi-file table. Pick per situation:

| Method | Globs? | Needs creds? | Use when |
|---|---|---|---|
| **A. S3 API** (`s3://rete/…/*.parquet`) | ✅ yes | yes (R2 keys) | you have the keys — the ergonomic default |
| **B. Manifest list** (`read_parquet([urls])`) | n/a | no | public / credential-less access |
| **C. Direct URL** (`https://…/file.parquet`) | n/a | no | single-file tables (e.g. `ror`) |

---

## A. S3 API — globbing works (recommended)

Point DuckDB at the R2 S3 endpoint once, then use `s3://rete/…` with wildcards.
Credentials live in the repo's gitignored `.env` (`ACCESS_KEY_ID`,
`SECRET_ACCESS_KEY`, `S3_API_ENDPOINT`) — never hard-code them.

```sql
INSTALL httpfs; LOAD httpfs;
SET s3_endpoint   = '<account>.r2.cloudflarestorage.com';  -- S3_API_ENDPOINT, host only, no https://
SET s3_region     = 'auto';
SET s3_url_style  = 'path';
SET s3_access_key_id     = '…';   -- ACCESS_KEY_ID
SET s3_secret_access_key = '…';   -- SECRET_ACCESS_KEY

SELECT count(*) FROM read_parquet('s3://rete/orcid/parquet-summaries/person/*.parquet');
-- 25,048,058
```

Python (loads `.env` for you):

```python
import duckdb, os
env = {}
for line in open(".env", encoding="utf-8"):
    if "=" in line and not line.strip().startswith("#"):
        k, v = line.split("=", 1); env[k.strip()] = v.strip().strip('"').strip("'")
con = duckdb.connect()
con.execute("INSTALL httpfs; LOAD httpfs;")
con.execute(f"SET s3_endpoint='{env['S3_API_ENDPOINT'].split('://')[-1].rstrip('/')}';"
            "SET s3_region='auto'; SET s3_url_style='path';")
con.execute(f"SET s3_access_key_id='{env['ACCESS_KEY_ID']}';"
            f"SET s3_secret_access_key='{env['SECRET_ACCESS_KEY']}';")
con.sql("SELECT count(*) FROM read_parquet('s3://rete/datacite/parquet-2025/*.parquet')").show()
```

## B. Manifest-driven list — public, no credentials

Read the dataset's `_parquet_manifest.json`, build the URL list, hand it to
`read_parquet` (which accepts a **list** of URLs):

```python
import duckdb, json, urllib.request
def urls(prefix, subdir=""):
    m = json.load(urllib.request.urlopen(urllib.request.Request(
        f"https://data.graphplaza.com/{prefix}/_parquet_manifest.json",
        headers={"User-Agent": "Mozilla/5.0"})))
    return ["https://data.graphplaza.com/" + f["key"] for f in m["files"]
            if subdir in f["key"]]

con = duckdb.connect(); con.execute("INSTALL httpfs; LOAD httpfs;")
con.execute("SELECT count(*) FROM read_parquet(?)",
            [urls("orcid", "parquet-summaries/person/")])   # -> 25,048,058
```

Pure-SQL variant (DuckDB reads the manifest JSON itself):

```sql
INSTALL httpfs; LOAD httpfs;
SELECT count(*) FROM read_parquet(
  (SELECT list('https://data.graphplaza.com/' || f.key)
   FROM read_json('https://data.graphplaza.com/orcid/_parquet_manifest.json') t,
        unnest(t.files) AS f(f)
   WHERE f.key LIKE '%parquet-summaries/person/%')
);
```

## C. Direct URL — single-file tables

```sql
SELECT count(*) FROM read_parquet('https://data.graphplaza.com/ror/parquet/ror.parquet');  -- 111,068
```

---

## Dataset catalog (R2 paths, sizes, join keys)

Paths below are the `s3://rete/…` / `https://data.graphplaza.com/…` suffix. Use
method A's glob, or B/C for public access. **DOI, ORCID and ROR are the join keys**
that stitch these together (see Cross-dataset joins).

### opencitations — the crosswalk hub
- `opencitations/meta-v13.1.0/parquet/*.parquet` — **135,416,506** rows
- Key cols: `omid, doi, pmid, openalex, issn, isbn, pub_year, title, author, venue, publisher, type` (author/venue carry embedded `omid:`/`orcid:`).

### orcid — person ↔ work authority
| Table (glob) | Rows | Key cols |
|---|---|---|
| `orcid/parquet-summaries/person/*.parquet` | 25,048,058 | `orcid`, names, country, `keywords_json`, `external_ids_json` |
| `orcid/parquet-summaries/work/*.parquet` | 149,782,968 | `orcid, doi, title, journal_title, type, pub_year` |
| `orcid/parquet-summaries/affiliation/*.parquet` | 25,063,901 | `orcid`, org, `ror`… |
| `orcid/parquet-summaries/funding/*.parquet` | 1,838,095 | `orcid`, org, award |
| `orcid/parquet-activities/{work,affiliation,funding,peer_review,research_resource}/…` | up to 149,933,459 | fuller per-activity detail |

### datacite — DOI outputs + the PID Graph
| Table | Rows | Key cols |
|---|---|---|
| `datacite/parquet-2023/*.parquet` | 52,863,283 | `doi, publisher, publication_year, resource_type_general, creators_json, related_identifiers_json, …` |
| `datacite/parquet-2024/*.parquet` | 72,019,577 | + `citation_count, view_count, download_count, …` |
| `datacite/parquet-2025/*.parquet` | 108,468,906 | (same 51-col shape as 2024) |
| `datacite/parquet-links-2023/*.parquet` | 167,844,248 | `subj_id, obj_id, relation_type, source_id, subj_type, obj_type` |
| `datacite/parquet-links-may2025/*.parquet` | 592,958,301 | (same — the PID Graph edge list) |

### openaire — the big graph (⚠ two model versions present)
Use the **`2026/` set** (v11.1.1, current); a legacy top-level `parquet-*` (v3.0) is also on R2.
| Table (2026) | Rows |
|---|---|
| `openaire/2026/parquet-publication/*.parquet` | 218,421,450 |
| `openaire/2026/parquet-dataset/*.parquet` | 101,778,366 |
| `openaire/2026/parquet-otherresearchproduct/*.parquet` | 37,832,720 |
| `openaire/2026/parquet-software/*.parquet` | 730,346 |
| `openaire/2026/parquet-person/*.parquet` | 14,803,875 (`id, given_name, family_name, pid_orcid`) |
| `openaire/2026/parquet-organization/*.parquet` | 494,099 (`id, legal_name, country_code, pids_json`) |
| `openaire/2026/parquet-project/*.parquet` | 3,909,902 |
| `openaire/2026/parquet-person_authorship/*.parquet` | 234,256,888 (`person_id, product_id, rank`) |
| `openaire/2026/parquet-person_coauthorship/*.parquet` | 376,504,588 |
| `openaire/2026/parquet-relation/*.parquet` | **6,325,064,342** (`source_id, target_id, rel_name, rel_type`) |

### The rest
| Dataset | Table(s) | Rows | Notes |
|---|---|---|---|
| **ror** | `ror/parquet/*.parquet` | 111,068 | org authority; `id`, `ror_id` (`https://ror.org/…`) |
| **dblp** | `dblp/parquet/record/*.parquet` | 12,751,652 | CS pubs; has `doi` |
| | `dblp/parquet/authorship/*.parquet` | 33,755,620 | `key, pos, author, orcid` |
| **crossref** | *(no Parquet on R2 — only the `crossref.rete` graph + raw `.gz`)* | — | query via the `.rete` instead |
| **zenodo** | `zenodo/parquet-metadata/*.parquet` | 7,759,325 | `doi`, records |
| | `zenodo/parquet-biosyslit/*.parquet` | 2,013,749 | biodiversity subset |
| **gotriple** | `gotriple/parquet/*.parquet` | 6,074,813 | SSH pubs; `id, doi` |
| **cordis** | `cordis/parquet/*.parquet` | 2,840,501 | 28 per-class tables (subject + typed cols) |
| | `cordis/triples/*.parquet` | 26,363,545 | flat `subject,predicate,object,otype,lang,datatype,graph` |
| **epfl-infoscience** | `epfl-infoscience/parquet-{publication,person,journal,orgunit,event,patent,product,funding,fulltext}/…` | 192,451 (pubs) … | `uuid, handle, doi, orcid, sciper, metadata_json`; `-fulltext` has `text` |
| **epfl-graph** | `epfl-graph/parquet/Nodes_N_Concept/*.parquet` | 6,225,797 | `id, name` |
| | `epfl-graph/parquet/Edges_N_Concept_N_Concept_T_Embeddings/*.parquet` | 192,581,518 | `from_id, to_id, score` |
| | `epfl-graph/parquet/Data_N_Object_T_FullContent/*.parquet` | 853,148 | multilingual text |

---

## Nested JSON columns

Many tables keep the rich/nested parts as `*_json` **string** columns. DuckDB
parses them inline — no pre-flattening:

```sql
-- DataCite creators (array of objects) -> one row per creator, pull the ORCID
SELECT d.doi,
       c.value ->> 'name'                                   AS creator,
       c.value -> 'nameIdentifiers' -> 0 ->> 'nameIdentifier' AS name_id
FROM read_parquet('s3://rete/datacite/parquet-2024/*.parquet') d,
     unnest(from_json(d.creators_json, '["json"]')) AS c(value)
WHERE d.creators_json IS NOT NULL
LIMIT 20;

-- ORCID person keywords
SELECT orcid, json_extract_string(external_ids_json, '$') AS ids
FROM read_parquet('s3://rete/orcid/parquet-summaries/person/*.parquet')
WHERE external_ids_json IS NOT NULL LIMIT 5;
```

Handy operators: `->` (JSON child), `->>` (child as text), `json_extract(col,'$.a.b')`,
`unnest(from_json(col,'["json"]'))` to explode a JSON array into rows.

---

## Single-dataset examples

```sql
-- opencitations: publications per type
SELECT type, count(*) n
FROM read_parquet('s3://rete/opencitations/meta-v13.1.0/parquet/*.parquet')
GROUP BY type ORDER BY n DESC LIMIT 10;

-- datacite: most-cited DOIs in 2024
SELECT doi, title, citation_count
FROM read_parquet('s3://rete/datacite/parquet-2024/*.parquet')
WHERE citation_count IS NOT NULL
ORDER BY citation_count DESC LIMIT 10;

-- datacite PID Graph: outgoing relations of a DOI
SELECT relation_type, obj_id
FROM read_parquet('s3://rete/datacite/parquet-links-may2025/*.parquet')
WHERE subj_id = '10.6084/m9.figshare.52754.v1';

-- orcid: a researcher's works
SELECT title, journal_title, pub_year, doi
FROM read_parquet('s3://rete/orcid/parquet-summaries/work/*.parquet')
WHERE orcid = '0000-0002-1825-0097' ORDER BY pub_year DESC;

-- openaire: publications per country of the responsible org (2026 relations)
SELECT count(*) FROM read_parquet('s3://rete/openaire/2026/parquet-publication/*.parquet');
```

---

## Cross-dataset joins — the scholarly graph in SQL

The whole point of the hub: the same **DOI / ORCID / ROR** appears across
datasets, so you can join them. Lowercase DOIs on join.

```sql
-- 1) A DOI seen by OpenCitations, DataCite AND ORCID (three-way crosswalk)
WITH oc AS (SELECT lower(doi) doi, title FROM read_parquet('s3://rete/opencitations/meta-v13.1.0/parquet/*.parquet') WHERE doi IS NOT NULL),
     dc AS (SELECT lower(doi) doi, publisher FROM read_parquet('s3://rete/datacite/parquet-2024/*.parquet') WHERE doi IS NOT NULL),
     ow AS (SELECT DISTINCT lower(doi) doi, orcid FROM read_parquet('s3://rete/orcid/parquet-summaries/work/*.parquet') WHERE doi IS NOT NULL)
SELECT oc.doi, oc.title, dc.publisher, ow.orcid
FROM oc JOIN dc USING (doi) JOIN ow USING (doi) LIMIT 20;

-- 2) ORCID authors -> their DataCite datasets (person ↔ output by DOI)
SELECT p.orcid, p.given_names, p.family_name, count(*) AS datacite_works
FROM read_parquet('s3://rete/orcid/parquet-summaries/person/*.parquet') p
JOIN read_parquet('s3://rete/orcid/parquet-summaries/work/*.parquet') w USING (orcid)
JOIN read_parquet('s3://rete/datacite/parquet-2025/*.parquet') d ON lower(w.doi) = lower(d.doi)
GROUP BY 1,2,3 ORDER BY datacite_works DESC LIMIT 20;

-- 3) ROR org authority -> OpenAIRE organizations by ROR id (in pids_json)
SELECT r.name, r.country_name, o.id AS openaire_org
FROM read_parquet('s3://rete/ror/parquet/ror.parquet') r
JOIN read_parquet('s3://rete/openaire/2026/parquet-organization/*.parquet') o
  ON o.pids_json LIKE '%' || r.id || '%'
LIMIT 20;

-- 4) DBLP (CS) DOIs that DataCite also holds
SELECT count(*) FROM read_parquet(?) db          -- dblp record file list (manifest)
JOIN read_parquet('s3://rete/datacite/parquet-2025/*.parquet') d
  ON lower(db.doi) = lower(d.doi);
```

> Scale note: `openaire/2026/parquet-relation` is **6.3 B rows** and DataCite links
> ~761 M — filter (a DOI, a year, a `rel_name`) before wide aggregates so DuckDB
> prunes row-groups and fetches only the ranges it needs. `SET threads` and
> `SET memory_limit` to taste; add `SET enable_object_cache=true` to reuse
> footers across queries in a session.

---

## See also
- `data/scholar/README.md` — the alignment hub (`scholar.ttl`) and canonical-IRI policy.
- Each dataset's `<prefix>/_parquet_manifest.json` (file list) and, on the HF bucket
  `katospiegel/rete-public/sources/<dataset>/`, its `schema.json` + `croissant.jsonld` + `.ttl`.
- The `.rete` graphs on R2 (`<prefix>/<prefix>.rete`) for the same data as a
  range-queryable RDF graph (SPARQL) instead of SQL.
