# Getting Started

This guide will walk you through the entire `rete` workflow: building a graph file, querying it locally, and publishing it to a URL for serverless querying.

## 1. Prerequisites: Everything Runs in Docker

To ensure reproducible builds and avoid toolchain headaches, `rete` is developed and built **entirely inside a Docker container**. You don't need to install Rust or WebAssembly tools on your host machine.

Open the project folder in your terminal and run:
```sh
# Build the dev container
docker build -t rete-dev -f .devcontainer/Dockerfile .

# Launch into the container shell
docker run --rm -it -v "${PWD}:/work" -w /work rete-dev bash

# Once inside, compile the CLI:
cargo build --release -p rete-cli
```
*Note: The compiled binary will be located at `target/release/rete`. Ensure it is in your `PATH` or use `cargo run -p rete-cli --` for the examples below.*

## 2. Building a `.rete` File

<figure class="fig-right">
  <img src="img/build-pipeline.svg" alt="A pipeline: .nt, .ttl and .nq inputs feed into 'rete build', which produces one social.rete file containing a dictionary, indexes and a pyramid, ready to put on an HTTP host or URL.">
  <figcaption><code>rete build</code> packs your triples into one immutable file — dictionary, permutation indexes, and a community pyramid.</figcaption>
</figure>

The `rete build` command takes raw RDF data (N-Triples, N-Quads, Turtle, or RDF/XML) and compiles it into a highly optimized, queryable `.rete` file.

**Basic Usage:**
```sh
# Build from a single file
rete build data.nt -o data.rete

# Merge multiple files together
rete build part1.nt part2.nt -o merged.rete

# Stream data from a URL
curl -s https://host/data.nt | rete build - -o data.rete
```

### Adding Metadata (Dataset Cards)
You can embed a "Dataset Card" directly into the file. This makes your data self-describing, carrying its title, license, and source alongside the data itself:
```sh
rete build chebi.owl -o chebi.rete --card \
  --title "ChEBI Ontology" --license "CC BY 4.0"
```

### Enable Full-Text Search
If you want to search for words *anywhere* inside literal values (not just label prefixes), enable the text index:
```sh
rete build data.nt -o data.rete --text-index
```

## 3. Querying Locally

Once you have a `.rete` file, you can query it immediately.

**Simple Pattern Matching:**
```sh
# Find everything that "knows" something
rete query data.rete --predicate '<http://ex/knows>'
```

**Full SPARQL Queries:**
```sh
# Output results as a formatted table
rete sparql data.rete "PREFIX e: <http://ex/> SELECT ?x ?z WHERE { ?x e:knows ?y . ?y e:knows ?z }"

# Output results as JSON (great for piping to other tools)
rete sparql data.rete "SELECT ?s ?p ?o WHERE { ?s ?p ?o } LIMIT 10" --json
```

## 4. Validating Data (SHACL)

Use `rete shacl` to ensure your data conforms to expected shapes and business logic. It reads a standard SHACL Turtle file and exits with a non-zero code if the graph is invalid.
```sh
rete shacl data.rete --shapes shapes.ttl
```

## 5. Inspecting a File

`rete` includes several tools to help you understand a graph without writing complex SPARQL queries.

**Basic File Info:**
```sh
rete info data.rete    # View the file header and metadata
rete stats data.rete   # View size, counts, top predicates, and entity shapes
rete search data.rete "alan" # Fast autocomplete search over entity labels
```

**Index-Free Summaries:**
These commands leverage the graph's pre-built summary pyramid, answering questions *without* scanning the raw data indexes:
```sh
rete summary data.rete    # Structural overview (Louvain communities)
rete schema data.rete     # Semantic overview (How rdf:type classes relate)
rete predicates data.rete # Exact counts for every predicate
```

## 6. Deploying and Querying Over a URL

The true power of `rete` is that it is **serverless**. A `.rete` file is immutable and self-contained. 

You can upload it to any static web host that supports HTTP `Range` requests (like AWS S3, GitHub Pages, or Cloudflare R2) and query it remotely!

**Test with a local server:**
```sh
python3 scripts/range_server.py 8000 .
rete query-url http://127.0.0.1:8000/data.rete --object '<http://ex/Dave>'
```

**Query against real HTTPS hosts:**
```sh
# Notice how fast this returns, even for massive remote files!
rete query-url https://my-bucket.s3.amazonaws.com/data.rete --predicate '<http://ex/knows>'
```
*How it works: `query-url` reads the remote dictionary, determines the exact byte-range needed for your query, and downloads only those specific bytes.*

## 7. Generating Synthetic Test Data

Need a massive graph to stress-test your system? Use our synthetic data generator to create realistic knowledge graphs featuring power-law citations, communities, and noisy data.

```sh
# Generate 10,000 synthetic papers (~315k triples)
uv run python scripts/synth_graph.py --papers 10000 -o clean.nt

# Build the graph
rete build clean.nt -o clean.rete
```

You can scale this linearly. To generate a **1 GB graph** (approx 12.5M triples), simply increase the `--papers` flag to `400000`.

## Next Steps
- Learn how to host your graphs securely in **[Hosting your .rete](hosting.md)**.
- Explore live examples in the **[Playground](playground-guide.md)**.
