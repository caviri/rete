#!/usr/bin/env python3
"""Fast companions for ethz-research-collection, built in DuckDB (vectorized).

Parses the N-Triples in SQL (not Python), then writes:
  * flat triples  -> companions/parquet/triples.parquet   (lossless: s,p,o,otype,value,datatype,lang)
  * per-class entity tables (wide, readable) -> companions/parquet/{table}.parquet
      works, persons, files, journals, grants, units
  * a single DuckDB file bundling every table -> companions/ethz-research-collection.duckdb
  * a small SQLite of the entity tables (not the 11.6M-row flat one) -> ...sqlite

Requires duckdb. Run in Docker.
"""
import duckdb
import os
import sys

NT = "data/ethz-research-collection/ethz.nt"
OUT = "data/ethz-research-collection/companions"
PQ = f"{OUT}/parquet"
E = "https://w3id.org/rete/ethz#"
DCT = "http://purl.org/dc/terms/"
RDFS = "http://www.w3.org/2000/01/rdf-schema#"
BIBO = "http://purl.org/ontology/bibo/"
RDF = "http://www.w3.org/1999/02/22-rdf-syntax-ns#"

os.makedirs(PQ, exist_ok=True)
con = duckdb.connect(f"{OUT}/ethz-research-collection.duckdb")
con.execute("PRAGMA threads=4")

print("parsing N-Triples in SQL ...", flush=True)
# Each line read as ONE column (delimiter that never occurs), then regex-split.
con.execute(f"""
CREATE OR REPLACE TABLE raw AS
SELECT line FROM read_csv('{NT}', columns={{'line':'VARCHAR'}}, delim='\\x01',
                          header=false, quote='', escape='', ignore_errors=true)
WHERE line IS NOT NULL AND length(line) > 4;
""")
con.execute(r"""
CREATE OR REPLACE TABLE triples AS
WITH p AS (
  SELECT
    regexp_extract(line, '^<([^>]*)>', 1) AS subject,
    regexp_extract(line, '^\S+\s+<([^>]*)>', 1) AS predicate,
    -- object = everything between predicate and the final ' .'
    regexp_extract(line, '^\S+\s+\S+\s+(.*) \.\s*$', 1) AS obj
  FROM raw
)
SELECT
  subject, predicate, obj AS object,
  CASE WHEN obj LIKE '<%>' THEN 'iri'
       WHEN obj LIKE '"%' THEN 'literal' ELSE 'other' END AS otype,
  CASE WHEN obj LIKE '<%>' THEN substr(obj, 2, length(obj)-2)
       WHEN obj LIKE '"%' THEN regexp_extract(obj, '^"(.*)"(\^\^<[^>]*>|@[A-Za-z-]+)?$', 1)
       ELSE obj END AS value,
  CASE WHEN obj LIKE '%"^^<%>' THEN regexp_extract(obj, '\^\^<([^>]*)>$', 1) ELSE NULL END AS datatype,
  CASE WHEN regexp_matches(obj, '"@[A-Za-z-]+$') THEN regexp_extract(obj, '@([A-Za-z-]+)$', 1) ELSE NULL END AS lang
FROM p WHERE subject <> '' AND predicate <> '';
""")
n = con.execute("SELECT count(*) FROM triples").fetchone()[0]
print(f"  {n:,} triples parsed", flush=True)

# helper: one (subject) row per entity of a class, pulling common predicates as
# scalar columns (MAX picks any one value; MIN/list where multi is expected).
def entity_table(name, cls, cols):
    # cols: list of (colname, predicate, agg) ; agg in ('any','list')
    sel = ["s.subject AS entity"]
    joins = []
    for i, (cn, pred, agg) in enumerate(cols):
        a = f"c{i}"
        if agg == "list":
            joins.append(f"LEFT JOIN (SELECT subject, list(value) AS v FROM triples WHERE predicate='{pred}' GROUP BY subject) {a} ON {a}.subject=s.subject")
            sel.append(f"{a}.v AS {cn}")
        else:
            joins.append(f"LEFT JOIN (SELECT subject, max(value) AS v FROM triples WHERE predicate='{pred}' GROUP BY subject) {a} ON {a}.subject=s.subject")
            sel.append(f"{a}.v AS {cn}")
    q = f"""CREATE OR REPLACE TABLE {name} AS
      SELECT {', '.join(sel)}
      FROM (SELECT DISTINCT subject FROM triples WHERE predicate='{RDF}type' AND object='<{cls}>') s
      {' '.join(joins)};"""
    con.execute(q)
    cnt = con.execute(f"SELECT count(*) FROM {name}").fetchone()[0]
    con.execute(f"COPY {name} TO '{PQ}/{name}.parquet' (FORMAT parquet, COMPRESSION zstd)")
    print(f"  {name}: {cnt:,} rows", flush=True)
    return name

print("building entity tables ...", flush=True)
tables = []
# works: pull the common scalar fields + list of subjects/authors
con.execute(f"""CREATE OR REPLACE TABLE works AS
SELECT s.subject AS work,
  t_title.v AS title, t_type.v AS type, t_year.v AS issued, t_doi.v AS doi,
  t_handle.v AS handle, t_avail.v AS availability, t_lang.v AS language,
  t_pub.v AS publisher, t_lic.v AS license, t_abs.v AS abstract,
  t_auth.v AS authors, t_subj.v AS subjects
FROM (SELECT DISTINCT subject FROM triples WHERE predicate='{RDF}type'
        AND object IN (SELECT '<'||o||'>' FROM (VALUES
          ('{E}JournalArticle'),('{E}ConferencePaper'),('{E}DoctoralThesis'),
          ('{E}MasterThesis'),('{E}BookChapter'),('{E}Report'),('{E}WorkingPaper'),
          ('{E}ReviewArticle'),('{E}Presentation'),('{E}Monograph'),('{E}Dataset'),
          ('{E}OtherPublication'),('{E}Publication'),('{E}ResearchData')) v(o))) s
LEFT JOIN (SELECT subject, max(value) v FROM triples WHERE predicate='{RDFS}label' GROUP BY subject) t_title ON t_title.subject=s.subject
LEFT JOIN (SELECT subject, max(value) v FROM triples WHERE predicate='{E}publicationType' GROUP BY subject) t_type ON t_type.subject=s.subject
LEFT JOIN (SELECT subject, max(value) v FROM triples WHERE predicate='{DCT}issued' GROUP BY subject) t_year ON t_year.subject=s.subject
LEFT JOIN (SELECT subject, max(value) v FROM triples WHERE predicate='{E}doi' GROUP BY subject) t_doi ON t_doi.subject=s.subject
LEFT JOIN (SELECT subject, max(value) v FROM triples WHERE predicate='{E}handle' GROUP BY subject) t_handle ON t_handle.subject=s.subject
LEFT JOIN (SELECT subject, max(value) v FROM triples WHERE predicate='{E}availability' GROUP BY subject) t_avail ON t_avail.subject=s.subject
LEFT JOIN (SELECT subject, max(value) v FROM triples WHERE predicate='{DCT}language' GROUP BY subject) t_lang ON t_lang.subject=s.subject
LEFT JOIN (SELECT subject, max(value) v FROM triples WHERE predicate='{DCT}publisher' GROUP BY subject) t_pub ON t_pub.subject=s.subject
LEFT JOIN (SELECT subject, max(value) v FROM triples WHERE predicate='{DCT}license' GROUP BY subject) t_lic ON t_lic.subject=s.subject
LEFT JOIN (SELECT subject, max(value) v FROM triples WHERE predicate='{DCT}abstract' GROUP BY subject) t_abs ON t_abs.subject=s.subject
LEFT JOIN (SELECT subject, list(object) v FROM triples WHERE predicate='{E}creator' GROUP BY subject) t_auth ON t_auth.subject=s.subject
LEFT JOIN (SELECT subject, list(value) v FROM triples WHERE predicate='{DCT}subject' GROUP BY subject) t_subj ON t_subj.subject=s.subject;""")
wc = con.execute("SELECT count(*) FROM works").fetchone()[0]
con.execute(f"COPY works TO '{PQ}/works.parquet' (FORMAT parquet, COMPRESSION zstd)")
print(f"  works: {wc:,} rows", flush=True)
tables.append("works")

tables.append(entity_table("persons", f"{E}Person", [
    ("name", f"{RDFS}label", "any"), ("orcid", f"{E}orcid", "any")]))
tables.append(entity_table("files", f"{E}File", [
    ("name", f"{RDFS}label", "any"), ("mime", f"{DCT}format", "any"),
    ("size_bytes", f"{E}sizeBytes", "any"), ("checksum", f"{E}checksum", "any"),
    ("checksum_algorithm", f"{E}checksumAlgorithm", "any"),
    ("download_url", f"{E}downloadURL", "any"), ("bundle", f"{E}bundle", "any")]))
tables.append(entity_table("journals", f"{E}Journal", [
    ("title", f"{RDFS}label", "any"), ("issn", f"{E}issn", "any")]))
tables.append(entity_table("grants", f"{E}Grant", [
    ("name", f"{RDFS}label", "any"), ("identifier", f"{DCT}identifier", "any"),
    ("program", f"{E}program", "any")]))
tables.append(entity_table("units", f"{E}OrgUnit", [
    ("name", f"{RDFS}label", "any"), ("leitzahl_code", f"{E}leitzahlCode", "any")]))

# flat lossless triples parquet
con.execute(f"COPY triples TO '{PQ}/triples.parquet' (FORMAT parquet, COMPRESSION zstd)")
print("  wrote flat triples.parquet", flush=True)

# small SQLite of the entity tables (skip the 11.6M flat table — too big/slow)
con.execute("INSTALL sqlite; LOAD sqlite;")
con.execute(f"ATTACH '{OUT}/ethz-research-collection.sqlite' AS lite (TYPE sqlite);")
for t in tables:
    con.execute(f"CREATE TABLE lite.{t} AS SELECT * FROM {t}")
con.execute("DETACH lite;")
print("done.", flush=True)
