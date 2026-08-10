# CLI Reference

Welcome to the `rete` command-line interface! The CLI (from the `rete-cli` crate) is your Swiss Army knife for building, querying, and managing `.rete` datasets. 

> **Tip:** Run `rete <command> --help` anytime for the authoritative list of flags. 
> Note: Terms should be written as canonical N-Triples tokens (e.g., `<http://ex/Alice>` for an IRI, `"30"` for a literal).

**A Note on JSON:** When you use `--json`, Rete-specific responses will include `"schemaVersion": 1` (covering Dataset Cards, search, cost, SHACL, etc.). Standard SPARQL Results JSON and JSON-LD responses will retain their standard shapes without this field.

---

## 1. Building and Preparing Data

### `rete build <inputs…> -o <out.rete>`
Builds a highly compressed `.rete` file from one or more RDF inputs. All inputs are merged under a single shared dictionary.

**Usage:**
```sh
# Build from local files
rete build a.nt b.nt -o merged.rete

# Stream from the web and build directly
curl -s https://host/data.nt | rete build - -o data.rete
```

**Key Flags:**
- `--format nt|nq|ttl|rdfxml`: Forces a specific format. (Normally auto-detected by file extension). Use `-` to read from stdin.
- `--card` (and `--card-file`, `--title`, `--license`, etc.): Embeds a [Dataset Card](dataset-cards.md) containing catalog metadata and an auto-derived profile.
- `--materialize`: Bakes RDFS/OWL-RL logic (entailments) directly into the file at build time.
- `--reason`: Runs the reasoner to check coherence, but instead of adding triples, it stamps the **coherence verdict** into the Dataset Card.
- `--text-index`: Adds a full-text search index over your literals (enables `rete search --contains`).
- `--pyramid-algo louvain|types`: Choose how communities are clustered for the schema pyramid (`louvain` is topological; `types` partitions by `rdf:type` for massive datasets). Set `--no-pyramid` to skip this step entirely for a smaller file.

### `rete repyramid <file> -o <out.rete>`
Rebuilds a file's community pyramid **in place** without needing to unpack and re-import the N-Quads. Perfect for adding a Dataset Card or a text index to an older file.

**Usage:**
```sh
# Add full-text search to an existing file
rete repyramid old.rete -o new.rete --text-index    

# Re-card a published file
rete repyramid old.rete -o new.rete --card --card-file curated.json --pyramid-algo types
```
> **Performance Note:** This command loads all statements into RAM. It safely handles up to ~80 million statements on a 48 GB machine. For larger files, use standard `export` piped into `build`.

---

## 2. Validating and Inspecting

### `rete validate <inputs…>`
Quickly parse and validate RDF files for syntax errors without building a `.rete` file.
```sh
rete validate data.ttl
```

### `rete info <file>`
Prints the decoded 1 KB header and the Dataset Card. It is lightning fast because it only reads tiny byte ranges, even on 50 GB files!

### `rete stats <file>`
Displays a human-friendly overview of your dataset: file size, triple counts, distinct terms, named graphs, pyramid levels, and top predicates.

### `rete verify <file>`
Recomputes the blake3 content hash and checks it against the header to detect file corruption.

### `rete search <file>`
Find entities instantly using two optimized modes:
1. **Label Prefix (Default):** Autocomplete search across `rdfs:label`, `skos:prefLabel`, `schema:name`, etc. Uses binary search, no full scans required!
2. **Full-Text (`--contains`):** Searches for words across all literals (requires a file built with `--text-index`).

```sh
rete search data.rete gluc                       # Label prefix (autocomplete)
rete search data.rete --contains glucose         # Literals containing "glucose"
rete search data.rete --contains glucose phosphate  # AND search for both words
```

### `rete card <file>`
Extracts the embedded Dataset Card.
- Try `--json` for raw output.
- Try `--format croissant` to project the card into the Croissant ML format (use `--sha256` to make it validator-clean).

### `rete graphs <file>`
Lists all named-graph IRIs in the dataset.

### `rete export <file>`
Dumps the `.rete` dataset back to text. 
- `--format nq` (Default) is a lossless round-trip of all graphs.
- `--format ttl` or `jsonld` will only export the **default graph**.

---

## 3. Querying 

### `rete query <file>` and `rete bgp <file>`
Evaluate simple triple patterns or Basic Graph Patterns without writing full SPARQL.
```sh
rete query data.rete --predicate '<http://ex/knows>'
rete bgp data.rete "?x <http://ex/knows> ?y . ?y <http://ex/knows> ?z"
```

### `rete why <file>` and `rete why-url <url>`
Explain the physical provenance of your results. Shows matched terms, dictionary IDs, the chosen index permutation, and exact file byte ranges! `why-url` does this for remote files.

### `rete sparql <file> "<query>"`
Run full SPARQL queries (`SELECT`, `ASK`, `CONSTRUCT`, `DESCRIBE`). 
- Add `--entail` to enable **OWL 2 QL reasoning** on the fly without materializing data. 
- Very memory efficient: large aggregations stream locally, handling massive datasets smoothly.

```sh
rete sparql data.rete "SELECT ?o WHERE { ?o a <…/Aves> }" --entail
```

### `rete serve <file>`
Turn your `.rete` file into a live **SPARQL 1.1 Protocol endpoint**. 
- It handles queries and `SPARQL UPDATE` operations. 
- Updates never mutate the base `.rete` file; they append to a plaintext journal (`.changes`).
- Use `GET /snapshot.rete` to download the live updated graph!

```sh
rete serve notes.rete --bind 127.0.0.1:7878 --token mysecret
```

### `rete cost <file-or-url> "<query>"`
Preview the byte and range-request cost of a SPARQL query *without actually evaluating it*. It tells you exactly how the query planner intends to route your query (e.g., summary-only vs full-index).

### `rete progressive <file-or-url> "<query>"`
Answers simple aggregation queries (like counts) by reading *only* the pyramid summary, skipping the massive triple index entirely.

### `rete cypher <file> "<query>"`
Prototype support for running a read-only **Cypher subset**.
```sh
rete cypher deps.rete "MATCH (a)-[:dependsOn*]->(b) WHERE b = <http://ex/log4x> RETURN a"
```

---

## 4. Reasoning & Shape Validation

### `rete reason [<file> | --url <url>]`
Run the RDFS/OWL reasoner to find logical inconsistencies. 
- Use `--check` in CI pipelines as a strict coherence gate (exits non-zero on inconsistencies).
- Use `--materialize` to output the base + inferred graph.
- Works natively over remote URLs!

### `rete shacl <file> --shapes <shapes.ttl>`
Validate your graph against SHACL Core shapes to guarantee data quality.

### `rete shacl-url <url> --shapes <shapes.ttl>`
Validate a remote graph lazily! The engine fetches *only the nodes targeted by your shapes*, ensuring you never download the entire dataset for a targeted check.

---

## 5. Analytics: Coarse Graphs and Traversal

### `rete summary <file>` & `rete schema <file>`
Extract high-level structural insights instantly. `summary` prints the community quotient graph and schema pyramid. `schema` prints the effective dataset schema (classes and predicates). Neither command reads the triple index!

### `rete communities <file>`
Extract the underlying Louvain communities. Great for topic modeling. Add `--profile` for basic topic profiles (top words, classes).

### `rete reach <file> --predicate <iri> --seed <iri>…`
Perform multi-source transitive reachability (e.g., dependency trees). 
- Use `--reverse` for impact analysis ("Who depends on this seed?").
- Use `--parallel` to max out your CPU cores for massive batch jobs.

---

## 6. Remote Files (HTTP Range Requests)

Any command ending in `-url` executes directly against a remote `http(s)://` host. **The engine fetches only the specific byte ranges required**, turning static storage (S3, R2, GitHub) into a live database.

- **`rete card-url <url>`**: Fetches the Dataset Card in exactly two small requests.
- **`rete summary-url <url>`**: Fetches the overview graph (skips the index).
- **`rete query-url <url>`**: Runs simple triple patterns remotely.
- **`rete sparql-url <url> "<query>"`**: Runs full SPARQL remotely. The ultimate lazy query engine!

### `rete card-audit <path|url>`
Audits the embedded starter queries within a Dataset Card to ensure they still produce valid results. Use `--measure` to execute them and record their byte costs, and `--write-costs` to save those metrics durably into the file!

---

## 7. Federation

### `rete federate <sources…> --query "<SPARQL>"`
Run a single SPARQL query across multiple `.rete` files (local or URLs) simultaneously. Results are unioned and deduplicated. The query planner automatically prunes sources that don't contain relevant predicates.

### `rete manifest <command>`
Manage a writable logical graph as a journaled log of immutable `.rete` segments. Commands include `init`, `add`, `status`, `query`, `seal`, and `compact`.

---

## 8. Exit Codes

- `0` — Success.
- `1` — Runtime, data, or network failure (e.g., malformed RDF, ignored `Range` headers, unsupported input).
- `2` — Command-line usage error.
- `3` — Data failure (e.g., SHACL validation failed, or reasoning found the graph incoherent).
