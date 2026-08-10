# Data Engineering & ETL

This page is for data engineers who want to integrate Rete with data lakes, generate tabular exports, ingest messy data, or work with genuinely massive datasets. 

*(If you just want to build your first file and query it, start at [Getting Started](getting-started.md).)*

---

## 1. Mini-Tutorial: CSV to `.rete` (ETL with Python)

Before diving into large-scale tools, let's walk through a classic Data Engineering task: taking a messy, real-world CSV file, converting it into RDF N-Triples, and building a `.rete` graph from it.

### Step 1: The Messy Data
Imagine you have an `employees.csv` file exported from a legacy system. It has inconsistent casing and some missing dates.

```csv
ID,Full Name,Department,Start Date
101,  jane smith, Engineering,2021-03-15
102,BOB JONES,Sales, 
103, alice wang,Engineering,2022-11-01
```

### Step 2: The ETL Script
We'll use Python with `pandas` to clean the data and emit **N-Triples**, the simplest and most robust RDF serialization format. Each line in an N-Triples file is simply `Subject Predicate Object .`.

Create a file called `etl_script.py`:

```python
import pandas as pd

# 1. Load the messy data
df = pd.read_csv("employees.csv")

# 2. Clean the data (Pandas shines here)
# Strip whitespace and fix casing
df['Full Name'] = df['Full Name'].str.strip().str.title()
df['Department'] = df['Department'].str.strip()
# Normalize dates
df['Start Date'] = pd.to_datetime(df['Start Date']).dt.strftime('%Y-%m-%d')

# 3. Define our vocabulary prefixes (for readability in the code)
EX = "http://example.org/vocab/"
EMP = "http://example.org/emp/"
DEPT = "http://example.org/dept/"
XSD = "http://www.w3.org/2001/XMLSchema#"
RDF = "http://www.w3.org/1999/02/22-rdf-syntax-ns#"

# 4. Generate N-Triples
with open("employees.nt", "w", encoding="utf-8") as out:
    for _, row in df.iterrows():
        # Construct IRIs (URIs enclosed in angle brackets)
        emp_iri = f"<{EMP}{row['ID']}>"
        dept_iri = f"<{DEPT}{row['Department'].replace(' ', '_')}>"
        
        # 4a. Emit Type
        out.write(f'{emp_iri} <{RDF}type> <{EX}Employee> .\n')
        
        # 4b. Emit String Literal
        out.write(f'{emp_iri} <{EX}name> "{row["Full Name"]}" .\n')
        
        # 4c. Emit Object Property (Link)
        out.write(f'{emp_iri} <{EX}worksIn> {dept_iri} .\n')
        
        # 4d. Emit Typed Literal (Date)
        if pd.notna(row['Start Date']):
            out.write(f'{emp_iri} <{EX}startDate> "{row["Start Date"]}"^^<{XSD}date> .\n')
```

> [!TIP]
> **Why string formatting?** While libraries like `rdflib` are great, manually formatting N-Triples strings in Python or DuckDB is often **10x to 100x faster** for massive datasets.

### Step 3: Run the ETL
Run the script to generate your `employees.nt` file:

```sh
python etl_script.py
```
*(Take a look at `employees.nt` — it's plain text, ready for any graph database).*

### Step 4: Build the Rete Graph
Now, feed those triples into the Rete CLI to build a queryable binary graph:

```sh
rete build employees.nt -o employees.rete
```

### Step 5: Query It
Your messy CSV is now a clean knowledge graph. Let's ask it a question:

```sh
rete query employees.rete -q "
  PREFIX ex: <http://example.org/vocab/>
  SELECT ?name ?date WHERE {
    ?emp ex:worksIn <http://example.org/dept/Engineering> ;
         ex:name ?name ;
         ex:startDate ?date .
  }
"
```

---

## 2. Lossless Entity Tables (The Best of Both Worlds)

The script `scripts/rdf_to_entity_tables.py` converts an RDF graph into a readable, one-table-per-type format **without losing any data**.

It outputs files as **Parquet**, **DuckDB**, and **SQLite** simultaneously.

### How it achieves losslessness:
- Standard properties (like birth date, occupation) become named `LIST` columns.
- A `types` column tracks all types assigned to the entity, ensuring entities are never duplicated across tables.
- An `extra` column (a map of predicate → objects) catches rare or multilingual properties.
- An `_untyped` residual table catches subjects with no type.
- Object values are stored as N-Triples strings (`<iri>`, `"literal"@en`), preserving datatypes perfectly.

**Command Example:**
```sh
uv run python scripts/rdf_to_entity_tables.py \
  --parts 1 --limit 12000000 --props 24 --min-entities 50 \
  -o data/ent --duckdb data/ent.duckdb --sqlite data/ent.sqlite --verify
```
*(The `--verify` flag automatically reconstructs the graph and checks it against the input to prove 100% losslessness).*

## 3. Property Tables (Columnar Comparison)

To compare how Rete performs against standard columnar layouts, you can use `scripts/rdf_to_property_tables.py`. This denormalizes Wikidata triples into **one Parquet table per entity type** (rows are entities, columns are properties). 

```sh
pip install --break-system-packages duckdb
uv run python scripts/rdf_to_property_tables.py --parts 10 --limit 120000000 -o data/wd-tables
```

> [!NOTE]
> This export is **not lossless**. Sparse properties become NULLs, and complex objects may be dropped. It is purely designed as a benchmarking companion to compare graph engines against columnar engines.

## 4. Virtual Knowledge Graphs (VKG)

When you generate the Parquet companions above, Rete also generates a `_manifest.parquet` file mapping every column back to its RDF predicate.

This manifest is exactly the **R2RML mapping** needed to power a **Virtual Knowledge Graph (VKG)** (like [Ontop](https://ontop-vkg.org/)). 
Instead of materializing the `.rete` graph, a VKG keeps the data in Parquet and rewrites SPARQL queries into SQL on-the-fly.

| Feature | Rete (Materialized) | Virtual KG (Ontop + DuckDB) |
|---|---|---|
| **Data Format** | Materialized into a `.rete` file | Remains as raw Parquet |
| **SPARQL Engine** | Native `.rete` evaluation | Rewritten to SQL via mappings |
| **Pros/Cons** | Graph-native, fast lookups, built-in reasoning & SHACL. Requires a build step. | Always fresh, no ingest required. Lacks graph-native features like path traversal. |

Both approaches use the same lazy HTTP-range fetching, so you can easily run both architectures side-by-side using the same storage bucket!

## 5. Fetching Real Data (Wikidata)

If you want to pull large chunks of real data, we provide two scripts:

### Slicing a Biology Graph via WDQS
`scripts/fetch_wikidata_bio.py` pulls a highly connected life-sciences slice from the live Wikidata Query Service (genes, proteins, diseases, and drugs).

```sh
# Pull ~40,000 triples
uv run python scripts/fetch_wikidata_bio.py --limit 4000 -o data/wikidata-bio.nt
rete build data/wikidata-bio.nt -o bio.rete
```

### Gigabyte-Scale Parsing from Dump
WDQS is too slow for massive pulls. `scripts/wikidata_parquet_to_nt.py` reads directly from the multi-gigabyte Wikidata Parquet dumps on Hugging Face using DuckDB.

```sh
uv run python scripts/wikidata_parquet_to_nt.py --limit 12000000 -o data/wd.nt
rete build data/wd.nt -o wd.rete
```
The script intelligently **recovers datatypes** that were stripped from the Parquet dump, ensuring dates become `xsd:dateTime` and coordinates become `geo:wktLiteral`.
