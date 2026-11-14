# Getting started

Build a small dependency graph, ask which packages depend on a library, and
inspect the description carried inside the file. You need Docker and an empty
working directory; no source checkout or Rust installation is needed.

## Set up the CLI {#getting-the-cli}

Use the [installation selector](install.md) to choose your operating system
and package. The Docker route below is a reproducible setup for this example;
if you already installed the native CLI, continue at [Build a graph](#building-a-rete-file).

The published CLI container runs the same commands as a local `rete` binary.
Define a wrapper in your current shell so that files in your working directory
are available inside the container.

This example pins the published 0.7.3 image, which writes format `0x08` and
reads `0x05` to `0x08`. Readers up to 0.6.x cannot open `0x08` files. See
[Compatibility](compatibility.md) before exchanging files with older readers.

On a POSIX shell (Linux or macOS):

```sh
alias rete='docker run --rm -i -v "$PWD:/data" -w /data ghcr.io/caviri/rete-cli:0.7.3'
```

On PowerShell:

```powershell
function rete { docker run --rm -i -v "${PWD}:/data" -w /data ghcr.io/caviri/rete-cli:0.7.3 @args }
```

`-v` shares your directory with `/data`, `-w` makes relative filenames work,
and `-i` allows piped input. Without `-i`, a piped build can silently receive
empty input and produce an empty graph. On Linux, add
`--user "$(id -u):$(id -g)"` if output files should belong to your user.
For Windows Git Bash, use PowerShell instead, or set `MSYS_NO_PATHCONV=1`
and mount `"$(pwd -W):/data"` to avoid MSYS rewriting container paths.

Prefer a language client? See [Python](python.md) or
[JavaScript](javascript.md). To build the CLI itself, follow
[Contributing](contributing.md).

## Build a graph you can check {#building-a-rete-file}

Our graph says that an application depends on a web library, which depends on
a logging library. The logging library has a vulnerability. Save these three
RDF statements as `data.nt`:

```nt
<http://ex/app> <http://ex/dependsOn> <http://ex/web> .
<http://ex/web> <http://ex/dependsOn> <http://ex/logging> .
<http://ex/logging> <http://ex/hasVulnerability> <http://ex/CVE-2099-0001> .
```

For a copy-and-paste setup in a POSIX shell:

```sh
cat > data.nt <<'EOF'
<http://ex/app> <http://ex/dependsOn> <http://ex/web> .
<http://ex/web> <http://ex/dependsOn> <http://ex/logging> .
<http://ex/logging> <http://ex/hasVulnerability> <http://ex/CVE-2099-0001> .
EOF
```

Or in PowerShell:

```powershell
@'
<http://ex/app> <http://ex/dependsOn> <http://ex/web> .
<http://ex/web> <http://ex/dependsOn> <http://ex/logging> .
<http://ex/logging> <http://ex/hasVulnerability> <http://ex/CVE-2099-0001> .
'@ | Set-Content -Encoding ascii data.nt
```

Now pack the RDF and include a Dataset Card:

```sh
rete build data.nt -o data.rete --card --title "My dependency graph"
```

The build reports **3 triples** and **6 terms**, and writes `data.rete` next to
`data.nt`. It also reports embedding the card, measuring starter queries, and
writing build information. Exact metadata sizes and timings can vary.

The file contains a dictionary and query indexes, plus a community summary
when built with a pyramid. It is an immutable snapshot: changing the RDF means
building another file. Check [Compatibility](compatibility.md) before exchanging files with older
readers; the file-format generation and project release version are separate.

<a id="a-full-ontology-rdfxml--an-optimized-rete"></a>
For your own data, the CLI also accepts N-Quads, Turtle, TriG, RDF/XML
(including RDF/XML `.owl` files), Jelly (`.jelly`), JSON-LD (`.jsonld`) and YAML-LD (`.yamlld`). N-Quads can preserve named graphs.
OWL/XML and Functional Syntax require conversion to RDF first. See
[CLI build options](cli.md) for input formats, merged inputs, streaming,
compression, memory limits and optional indexes.

## Ask a question and inspect the file {#querying-locally}

Which packages depend on `logging`, directly or through another package?
The `+` follows one or more `dependsOn` edges.

```sh
rete sparql data.rete "PREFIX e: <http://ex/> SELECT ?package WHERE { ?package e:dependsOn+ e:logging } ORDER BY ?package"
```

Expected output:

```text
2 solution(s)
?package=<http://ex/app>
?package=<http://ex/web>
```

`logging` itself is absent: this query needs at least one dependency edge.
[Query a .rete](#query-a-rete) below shows the other ways to ask.

<a id="inspecting-a-file"></a>
Inspect the description stored with the graph:

```sh
rete card data.rete
rete verify data.rete
```

The card begins with `Dataset Card`, title `My dependency graph`, **3 triples**,
**6 terms**, and the `http://ex/` vocabulary. Its predicate counts are **2** for
`dependsOn` and **1** for `hasVulnerability`. It also includes starter queries
and a separate build record. `verify` should report that the content hash
matches. See [Dataset Cards](dataset-cards.md) for curated metadata and build
provenance.

<a id="validating-shapes"></a>
Source syntax validation (`validate`), file integrity (`verify`) and
[SHACL shape validation](shacl.md) check different things.

## Query a .rete {#query-a-rete}

The same file answers the same SPARQL from the CLI, Python, JavaScript, a URL
and the browser. Each example below uses the graph you just built. For the
query language itself, see [SPARQL support](sparql.md); for joins, reasoning and
analysis, [Query and analyze](query-and-analyze.md).

### Python

```sh
pip install rete-graph==0.7.3
```

Save as `query.py`:

```python
import rete_graph as rete

g = rete.open("data.rete")
rows = g.query("""
    PREFIX e: <http://ex/>
    SELECT ?package WHERE { ?package e:dependsOn+ e:logging } ORDER BY ?package""")
for row in rows:
    print(row["package"].value)
print(g.quads, "triples")
```

```sh
python query.py
```

```text
http://ex/app
http://ex/web
3 triples
```

The package is `rete-graph`; the module is `rete_graph`. `query()` returns one
dict per row, with each value a `Term` that has `.value`. See [Python](python.md).

### JavaScript

```sh
npm install rete-graph@0.7.3
```

Save as `query.mjs`:

```js
import { readFileSync } from "node:fs";
import { open } from "rete-graph";

const g = await open(readFileSync("data.rete"));
const rows = g.query(`
  PREFIX e: <http://ex/>
  SELECT ?package WHERE { ?package e:dependsOn+ e:logging } ORDER BY ?package`);
for (const row of rows) console.log(row.package.value);
console.log(g.quads, "triples");
```

```sh
node query.mjs
```

```text
http://ex/app
http://ex/web
3 triples
```

This runs in Node 18 or later. In a browser, open the bytes in a worker or
fetch them yourself. See [JavaScript](javascript.md).

### Labels and words

A graph with `rdfs:label` literals can be searched by label prefix and by word.
Our dependency graph has none, so add three labels and build a second file with
both indexes:

```sh
cat > labels.nt <<'EOF'
<http://ex/app> <http://www.w3.org/2000/01/rdf-schema#label> "Shop app" .
<http://ex/web> <http://www.w3.org/2000/01/rdf-schema#label> "Web framework" .
<http://ex/logging> <http://www.w3.org/2000/01/rdf-schema#label> "Logging library" .
EOF
rete build data.nt labels.nt -o labelled.rete --card --title "My dependency graph" --label-index --text-index
```

`rete search` matches a label prefix, and `--contains` matches a word in any
literal:

```sh
rete search labelled.rete "Web"
rete search labelled.rete --contains library
```

```text
Web framework	<http://ex/web>
<http://ex/logging>
```

<a id="label-search-autocomplete"></a>
<a id="full-text-search-word--contains"></a>
The first command is what autocomplete uses; the second finds the subject by
content. Both flags are optional and cost file size; see the
[CLI reference](cli.md#full-text-index---text-index-and-its-scope).

### What a query will read

`rete cost` previews the range reads a query needs, without running it. It is
most useful on a large or remote file:

```sh
rete cost data.rete "PREFIX e: <http://ex/> SELECT ?package WHERE { ?package e:dependsOn+ e:logging }"
```

```text
query cost preview
  source: data.rete
  source kind: local
  file bytes: 18221
  current engine access: lazy-tiles
  query predicates: <http://ex/dependsOn>
  summary exact answer: unavailable (query shape is not exactly answerable from summary predicate totals)
  summary overview: 18221 bytes in 2 range request(s) · skips index
  routed pattern open: unavailable (query is not a single default-graph triple pattern)
  lazy query open: 18221 bytes in 1 range request(s) · reads index
  full query open: 18221 bytes in 2 range request(s) · reads index
  index section: offset 18014 · len 184
  note: SPARQL evaluation opens lazily (tile directories only) and range-fetches index tiles as the query touches them; the full query-open figure is the eager upper bound.
```

The graph is 18 KB, so the whole file is one read. On a large file the lazy
open reads the first 128 KiB and the query then reads only the tiles it needs.

### Several files

`rete federate` runs one query over several files, local paths and URLs mixed,
and merges the rows. Build a second file with one more dependency:

```sh
echo '<http://ex/api> <http://ex/dependsOn> <http://ex/logging> .' > more.nt
rete build more.nt -o more.rete
rete federate data.rete more.rete --query "PREFIX e: <http://ex/> SELECT ?package WHERE { ?package e:dependsOn e:logging }"
```

```text
  data.rete: 1 row(s) in 2.2ms
  more.rete: 1 row(s) in 2.5ms
2 solution(s)
federated 2 source(s): 2 queried, 0 pruned (routing on); 2 merged result(s)
?package=<http://ex/web>
?package=<http://ex/api>
```

Rows come in source order and are not re-sorted, because each file is queried on
its own: `ORDER BY`, `COUNT` and `LIMIT` apply per file. The CLI does a union
and no joins across files; the playground can join across sources. See
[Federated queries](federation.md).

### A file on a URL {#remote-query}

A remote file is queried through HTTP range requests, so nothing is downloaded
beyond what the query reads. To try it without a host, serve this directory with
the repository's range-aware dev server,
[`scripts/range_server.py`](https://github.com/caviri/rete/blob/main/scripts/range_server.py)
(Python's `http.server` ignores `Range`). Here it runs in a container, after
copying `range_server.py` into the directory:

```sh
docker run -d --name qs-range -p 8000:8000 -v "$PWD:/srv:ro" -w /srv python:3.12-slim python range_server.py 8000 /srv
rete sparql-url http://host.docker.internal:8000/data.rete "PREFIX e: <http://ex/> SELECT ?package WHERE { ?package e:dependsOn+ e:logging } ORDER BY ?package"
docker rm -f qs-range
```

(`host.docker.internal` is the host as seen from Docker Desktop. A native `rete`
uses `127.0.0.1`.)

```text
?package=<http://ex/app>
?package=<http://ex/web>
2 solution(s)
(fetched 18221 bytes in 1 range request(s); file is 18221 bytes)
```

The last line says what it fetched. This file is small, so it was read whole in
one request. On a 106 MB graph, the
[build tutorial](build-tutorial.md#8-use-the-file-over-http) shows a lookup that
read about 3% of it.

### The browser

The [playground](playground-guide.md) runs the same queries in a page, with no
install. Open a catalog dataset, or add your own file by URL under
**+ Add source**. A browser needs the host to send CORS headers as well as
honor `Range`; see [Hosting](hosting.md).

## Put the file on a URL {#deploying--querying-over-a-url}

Upload `data.rete` to a host that supports HTTP byte ranges, then query it as in
[A file on a URL](#remote-query), replacing the example address with yours:

```sh
rete sparql-url https://your-host.example/data.rete "PREFIX e: <http://ex/> SELECT ?package WHERE { ?package e:dependsOn+ e:logging } ORDER BY ?package"
```

Use [Hosting](hosting.md) to check range responses, caching and browser CORS.
The shared CLI/Python/R HTTP reader can retain a bounded whole-file response
from a host that ignores ranges; it refuses oversized responses. A range-capable
host is the reliable choice for large files.

<a id="going-further"></a>
Continue with the [playground](playground-guide.md), the
[dependency-impact scenario](scenario.md), the
[build tutorial](build-tutorial.md) (Python and CLI, the card, the file layout,
time and memory), or [tables and large builds](data-engineering.md).
<a id="building-from-source"></a>
<a id="generating-synthetic-test-data"></a>
<a id="scaling-to-1-gb"></a>
<a id="testing"></a>
Source builds, synthetic fixtures and test commands belong in
[Contributing](contributing.md).
