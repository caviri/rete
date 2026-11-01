# Trace a dependency vulnerability from a URL {#a-real-world-scenario-a-queryable-sbom-on-a-url}

Publish a software dependency graph as a `.rete` file, then ask which packages
are affected by a vulnerable dependency. The client runs the query; the host
only serves the file.

This example uses the repository's
[`examples/deps.nt`](https://github.com/caviri/rete/blob/main/examples/deps.nt).
It contains package types, these dependency edges, and one fictional CVE:

```text
app     -> web, auth
web     -> logging
auth    -> logging, safejson
logging -> log4x
log4x   -> hasVulnerability -> CVE-2099-0001
```

Commands below use `rete`; [Getting started](getting-started.md) explains the
published Docker wrapper for POSIX shells and PowerShell. Run them in a checkout
containing `examples/deps.nt`, or download that file into an `examples` directory.

## Build and check the snapshot {#1-validate-then-build}

Validate the RDF syntax, build a file, then check its content hash:

```sh
rete validate examples/deps.nt
rete build examples/deps.nt -o deps.rete
rete verify deps.rete
```

Validation reports **13 statements**, all in the default graph. The build
reports **13 triples**, **12 terms** and **2 pyramid levels**; verification
should say the content hash matches. File size can change between releases.
These checks establish syntax and file integrity, rather than completeness of
an SBOM or absence of vulnerabilities.

Before publishing, check the impact query locally:

```sh
rete sparql deps.rete "PREFIX e: <http://ex/> SELECT ?d WHERE { ?d e:dependsOn+ e:log4x } ORDER BY ?d"
```

It should return four packages, in order: `app`, `auth`, `logging`, `web`.
`safejson` has no dependency path to `log4x`, and `log4x` itself has no outgoing
dependency path to itself.

## Publish the file {#2-publish}

Upload the snapshot to a static host with HTTP range support. For example,
with a configured AWS CLI and your own bucket:

```sh
aws s3 cp deps.rete s3://my-bucket/deps.rete
```

The URL must be readable by your intended clients. Use [Hosting](hosting.md)
for permissions, range checks, CORS for browser access, and cache validators.
Publish changed content at a new versioned URL or configure validation so
clients do not reuse stale cached ranges. The file's content hash identifies
its content; HTTP caching policy still belongs to the host.

## Ask which packages are affected {#3-query-it-from-the-url}

<a id="the-headline-question-what-does-the-cve-impact"></a>
Replace the URL below with the location you published:

```sh
rete sparql-url https://my-bucket.s3.amazonaws.com/deps.rete "PREFIX e: <http://ex/> SELECT ?d WHERE { ?d e:dependsOn+ e:log4x } ORDER BY ?d"
```

Expected bindings are the same as the local query:

```text
?d=<http://ex/app>
?d=<http://ex/auth>
?d=<http://ex/logging>
?d=<http://ex/web>
```

The command also reports four solutions and its fetched bytes and requests.
The `+` follows one or more dependency edges. This toy graph may fit in one
bounded read. On larger graphs, selective queries can avoid much of the file,
but long paths and broad queries may read substantial index data. See
[SPARQL support](sparql.md) for lazy-query support and limits.

<a id="just-the-affected-libraries-with-the-cve-id"></a>
To restrict the answer to libraries and discover the vulnerability identifier
from the graph, join the path with its vulnerability and type statements:

```sh
rete sparql-url https://my-bucket.s3.amazonaws.com/deps.rete "PREFIX e: <http://ex/> SELECT ?lib ?cve WHERE { ?lib e:dependsOn+ ?v . ?v e:hasVulnerability ?cve . ?lib a e:Library } ORDER BY ?lib"
```

This returns `auth`, `logging` and `web`, each paired with
`http://ex/CVE-2099-0001`. `app` is an Application, so the type constraint
excludes it. This is dependency reachability in the supplied data, rather than
an assessment of exploitability or version ranges.

<a id="a-simple-point-lookup-triple-pattern-no-sparql"></a>
A direct fact lookup needs no SPARQL:

```sh
rete query-url https://my-bucket.s3.amazonaws.com/deps.rete --predicate '<http://ex/hasVulnerability>'
```

It returns the statement linking `log4x` to the CVE.
<a id="overview-first-without-downloading-the-index"></a>
For an overview, use `summary-url` on the URL, or `schema deps.rete` locally.
These summarize community structure and typed relations; they do not replace
the exact dependency-path answer. See [Semantic zoom](semantic-zoom.md) and
the [CLI reference](cli.md).

## Inspect the transport and open the browser {#4-querying-with-plain-curl}

A `.rete` URL is not a SPARQL endpoint: `curl` can retrieve bytes, while a rete
client interprets the file and runs queries. Check a published file's header
range with this POSIX command (use `curl.exe` in PowerShell):

```sh
curl -sD - -o header.bin -r 0-1023 https://my-bucket.s3.amazonaws.com/deps.rete
```

A range-capable host returns **206 Partial Content** and a `Content-Range`
header beginning `bytes 0-1023/`. The saved header begins with `RETE`.
A host returning `200` has sent a whole-file response. Current readers can
retain such a response up to 256 MiB; larger responses are refused rather than
silently downloading an unbounded file. Configure ranges for reliable large-file
access; details are in [Hosting](hosting.md).

For this small example you can also download the file and run the local query:

```sh
curl -s https://my-bucket.s3.amazonaws.com/deps.rete -o deps.rete
```

<a id="5-in-the-browser"></a>
In the [playground](playground.html), use **+ Add source > Connect (lazy)**
with the same URL and paste the impact query. With CORS enabled on the host,
the browser runs it against the same snapshot and shows the bytes fetched.
See the [playground guide](playground-guide.md) for interpreting results and
sharing the query. A custom dashboard can use the [Browser API](browser.md).
