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

This example pins the published 0.4.2 image, which writes format `0x05`.
Current source builds write `0x06`; the 0.4.2 reader cannot open those newer
files. See [Compatibility](compatibility.md) before exchanging files between
the released image and a source build.

On a POSIX shell (Linux or macOS):

```sh
alias rete='docker run --rm -i -v "$PWD:/data" -w /data ghcr.io/caviri/rete-cli:0.4.2'
```

On PowerShell:

```powershell
function rete { docker run --rm -i -v "${PWD}:/data" -w /data ghcr.io/caviri/rete-cli:0.4.2 @args }
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
For your own data, the CLI also accepts N-Quads, Turtle, TriG and RDF/XML
(including RDF/XML `.owl` files). N-Quads can preserve named graphs.
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
For triple lookups, joins, JSON results and supported SPARQL features, see
[CLI reference](cli.md) and [SPARQL support](sparql.md). Consult the support page for execution-mode differences before moving a
complex query between local files and URLs.

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
<a id="label-search-autocomplete"></a>
<a id="full-text-search-word--contains"></a>
For label autocomplete and word search, see `build --label-index`,
`build --text-index` and `search` in the [CLI reference](cli.md).

## Put the file on a URL {#deploying--querying-over-a-url}

Upload `data.rete` to a host that supports HTTP byte ranges. A remote query
uses the same SPARQL but reads the file through its URL:

```sh
rete sparql-url https://your-host.example/data.rete "PREFIX e: <http://ex/> SELECT ?package WHERE { ?package e:dependsOn+ e:logging } ORDER BY ?package"
```

Replace the example URL with your uploaded file. The two packages should be
the same; the command also reports fetched bytes and requests. A tiny graph
may be read in full. Larger graphs can benefit from selective range reads,
but the cost depends on the query and file layout.

Use [Hosting](hosting.md) to check range responses, caching and browser CORS.
The shared CLI/Python/R HTTP reader can retain a bounded whole-file response
from a host that ignores ranges; it refuses oversized responses. A range-capable
host is the reliable choice for large files.

<a id="going-further"></a>
Continue with the [playground](playground-guide.md), the
[dependency-impact scenario](scenario.md), or
[tables and large builds](data-engineering.md).
<a id="building-from-source"></a>
<a id="generating-synthetic-test-data"></a>
<a id="scaling-to-1-gb"></a>
<a id="testing"></a>
Source builds, synthetic fixtures and test commands belong in
[Contributing](contributing.md).
