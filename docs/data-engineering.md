# Data Engineering & Tables

This page is for data engineers who want to integrate Rete with data lakes, generate tabular exports, or ingest genuinely massive datasets. 

*(If you just want to build your first file and query it, start at [Getting Started](getting-started.md).)*

## 1. Lossless Entity Tables (The Best of Both Worlds)

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

## 2. Property Tables (Columnar Comparison)

To compare how Rete performs against standard columnar layouts, you can use `scripts/rdf_to_property_tables.py`. This denormalizes Wikidata triples into **one Parquet table per entity type** (rows are entities, columns are properties). 

```sh
pip install --break-system-packages duckdb
uv run python scripts/rdf_to_property_tables.py --parts 10 --limit 120000000 -o data/wd-tables
```

> [!NOTE]
> This export is **not lossless**. Sparse properties become NULLs, and complex objects may be dropped. It is purely designed as a benchmarking companion to compare graph engines against columnar engines.

## 3. Virtual Knowledge Graphs (VKG)

When you generate the Parquet companions above, Rete also generates a `_manifest.parquet` file mapping every column back to its RDF predicate.

This manifest is exactly the **R2RML mapping** needed to power a **Virtual Knowledge Graph (VKG)** (like [Ontop](https://ontop-vkg.org/)). 
Instead of materializing the `.rete` graph, a VKG keeps the data in Parquet and rewrites SPARQL queries into SQL on-the-fly.

| Feature | Rete (Materialized) | Virtual KG (Ontop + DuckDB) |
|---|---|---|
| **Data Format** | Materialized into a `.rete` file | Remains as raw Parquet |
| **SPARQL Engine** | Native `.rete` evaluation | Rewritten to SQL via mappings |
| **Pros/Cons** | Graph-native, fast lookups, built-in reasoning & SHACL. Requires a build step. | Always fresh, no ingest required. Lacks graph-native features like path traversal. |

Both approaches use the same lazy HTTP-range fetching, so you can easily run both architectures side-by-side using the same storage bucket!

## 4. Fetching Real Data (Wikidata)

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
