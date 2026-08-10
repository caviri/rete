# Federated Queries (Multi-File SPARQL)

The `rete federate` command allows you to run **a single SPARQL query across multiple `.rete` files** simultaneously. It seamlessly merges results from local files and remote `http(s)://` URLs. 

This is perfect when your dataset is sharded across multiple files and you want to query them as if they were one large graph.

```sh
# Basic usage
rete federate <source1> <source2>... --query "<SPARQL>" [--json] [--no-route]
```

> [!WARNING]
> **Limitations:** This is a union-based federation tool, not a distributed join engine. Read the [Limitations](#limitations) carefully. (If you want to federate a query to a live endpoint *from inside* your SPARQL query, use the `SERVICE` clause instead).

<figure class="fig-right">
  <img src="img/federation.svg" alt="A SPARQL query goes to a router that does predicate routing, fans out to three .rete files each with its own dictionary, then results are merged at the term level into a row set.">
  <figcaption>Each file keeps its own dictionary; the router sends each pattern only to files holding its predicate, then results merge at the term level.</figcaption>
</figure>

## How it Works (Term-Level Merging)

Because each `.rete` file has its own internal dictionary of integer IDs (meaning ID `42` in file A is completely different from ID `42` in file B), Rete **cannot** merge index files directly.

Instead, federation works at the **term (string) level**:
1. The query runs independently on each file.
2. The results are converted to strings.
3. The resulting rows are merged.

This approach is highly effective for datasets sharded by time or category.

## Merge Behavior

| Query Form  | How Results are Merged |
|-------------|------------------------|
| **`SELECT`**| **Union** of solution rows. Rows are deduplicated (identical rows collapse). |
| **`ASK`**   | **Logical OR** (Returns `true` if *any* source matches). |
| **`CONSTRUCT`**| **Union** of constructed triples, deduplicated. |

## Smart Routing & Pruning

By default, Rete acts smart to save time and bandwidth. Before evaluating a query, `federate` cheaply reads the summary of each source file to find out what **predicates** it contains. 

If your query asks for `foaf:knows` but a source file's summary shows it doesn't contain that predicate, Rete completely skips (prunes) that file without ever downloading its index.

- **`--no-route`**: Disables this smart pruning and forces Rete to query every file.
- If your query contains only variable predicates (e.g., `?s ?p ?o`), Rete cannot prune and will query every file.
- Files without a summary are never pruned.

## Limitations

- **No Cross-File Joins**: A query that needs a triple from File A joined with a triple from File B will **not** find a result. Each file is queried in absolute isolation.
- **Aggregates are Per-Source**: A federated `SELECT (COUNT(*) AS ?n)` will return *one count per source file*, not a combined global sum.
- **`LIMIT` is Per-Source**: `LIMIT 5` across 2 files will return up to **10** rows (5 from each).

## Real-World Example: OpenCitations Shards

Imagine you have two files containing citations of the AlphaFold paper, sharded by year (`cites-2021.rete` and `cites-2024.rete`).

```sh
rete federate data/opencitations/cites-2021.rete data/opencitations/cites-2024.rete \
  --query 'SELECT ?citing WHERE {
             ?citing <http://purl.org/spar/cito/cites>
                     <https://doi.org/10.1038/s41586-021-03819-2> } LIMIT 5'
```

Output:
```text
  data/opencitations/cites-2021.rete: 5 row(s) in 3.6ms
  data/opencitations/cites-2024.rete: 5 row(s) in 15.6ms
10 solution(s)
federated 2 source(s): 2 queried, 0 pruned (routing on); 10 merged result(s)
...
```

Notice that because of the `LIMIT 5`, Rete returned 5 rows from *each* shard, resulting in 10 total merged rows.

### Pruning in Action

If we throw an unrelated file into the mix (`other-demo.rete`) that doesn't contain the `cito:cites` predicate, Rete will safely ignore it:

```text
federated 3 source(s): 2 queried, 1 pruned (routing on); 6 merged result(s)
  pruned (predicate-disjoint): other-demo.rete
```

### Mixing Local Files and Web URLs

You can freely mix local files with `.rete` files hosted on the internet. Rete uses efficient HTTP Range requests to only download what it needs.

```sh
rete federate \
  https://data.graphplaza.com/worldcup2026/worldcup2026.rete \
  data/opencitations/cites-2024.rete \
  --query 'SELECT ?citing WHERE { ... }'
```

## Federation in the Playground

You can federate data right in your browser via the [Playground](playground.html). 

Just click the **+ Add source** button under the query editor. You can add datasets from the catalog, paste a URL to a remote `.rete` file, or even point to a live SPARQL endpoint.

When you run a query with multiple sources active, the playground intelligently fans the query out, merges the results, and displays a live progress counter. 

> [!TIP]
> **Cross-Source Joins in the Playground!**
> Unlike the CLI, the playground *can* perform **term-level cross-source JOINs** in the browser! It splits Basic Graph Patterns across sources and joins the partial rows in memory.
