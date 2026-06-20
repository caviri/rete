# A real-world scenario: a queryable SBOM on a URL

A worked end-to-end example: publish a software **dependency graph** (an
SBOM-style slice) as a single `.rete` file, then answer *"what is impacted by this
CVE?"* — over HTTP, fetching only the bytes the query needs, with no database.

The data ([`examples/deps.nt`](../examples/deps.nt)) models packages, their kind
(`rdf:type Application`/`Library`), `dependsOn` edges, and a vulnerable package:

```
<http://ex/app>     dependsOn  <http://ex/web>, <http://ex/auth>
<http://ex/web>     dependsOn  <http://ex/logging>
<http://ex/auth>    dependsOn  <http://ex/logging>, <http://ex/safejson>
<http://ex/logging> dependsOn  <http://ex/log4x>
<http://ex/log4x>   hasVulnerability  <http://ex/CVE-2099-0001>
```

## 1. Validate, then build

Check the source is well-formed RDF first, then pack it:

```sh
rete validate examples/deps.nt
#  valid: 13 statement(s) — 13 in the default graph, 0 named graph(s)

rete build examples/deps.nt -o deps.rete
#  wrote deps.rete: 13 triples, 12 terms, 2 pyramid level(s), 610 bytes

rete verify deps.rete        # confirm the content hash
#  OK — content hash matches
```

## 2. Publish

It's one immutable file — upload it anywhere that serves HTTP `Range` requests:

```sh
aws s3 cp deps.rete s3://my-bucket/deps.rete          # S3
# or commit it to a repo and use the raw URL, or any CDN / static host.
```

No server, no endpoint, no index to maintain. The file's blake3 content hash
doubles as an ETag-like id, so clients can cache byte ranges forever.

## 3. Query it from the URL

### The headline question: what does the CVE impact?

Everything that (transitively) depends on the vulnerable `log4x`:

```sh
rete sparql-url https://my-bucket.s3.amazonaws.com/deps.rete \
  "PREFIX e: <http://ex/> SELECT ?d WHERE { ?d e:dependsOn+ e:log4x }"
#  ?d=<http://ex/app>
#  ?d=<http://ex/auth>
#  ?d=<http://ex/logging>
#  ?d=<http://ex/web>
#  (fetched 606 bytes in 4 range request(s); file is 610 bytes)
```

`safejson` is correctly excluded — it doesn't reach the vulnerable package. The
client fetched the file in **4 bounded range requests**, never scanning linearly.

### Just the affected *libraries*, with the CVE id

```sh
rete sparql-url https://my-bucket.s3.amazonaws.com/deps.rete \
  "PREFIX e: <http://ex/>
   SELECT ?lib ?cve WHERE {
     ?lib e:dependsOn+ ?v . ?v e:hasVulnerability ?cve .
     ?lib a e:Library
   }"
```

### A simple point lookup (triple pattern, no SPARQL)

```sh
rete query-url https://my-bucket.s3.amazonaws.com/deps.rete \
  --predicate '<http://ex/hasVulnerability>'
```

### Overview first, without downloading the index

<img src="img/progressive-fetch.svg" alt="A client issues three small range requests for the header, dictionary, and pyramid summary; the large index block is greyed out and never fetched.">

*The overview path: header + dictionary + summary in three range reads (~25% of the file); the triple index stays on the server.*

```sh
rete summary-url https://my-bucket.s3.amazonaws.com/deps.rete   # community quotient graph
rete schema      deps.rete                                       # classes & relations (local)
```

## 4. Querying with plain `curl`

There is **no query endpoint** — a `.rete` query is a set of HTTP `Range` reads
that the `rete` client (or the WASM engine) performs. You can watch and even
drive those reads with `curl`.

**The whole file is tiny here; fetch it and query locally:**

```sh
curl -s https://my-bucket.s3.amazonaws.com/deps.rete -o deps.rete
rete sparql deps.rete "PREFIX e: <http://ex/> SELECT ?d WHERE { ?d e:dependsOn+ e:log4x }"
```

**See the range mechanics directly** — the first 1024 bytes are the header, and a
`Range` request returns `206 Partial Content`:

```sh
# Fetch just the header (bytes 0–1023); confirm the magic and partial response:
curl -s -r 0-1023 https://my-bucket.s3.amazonaws.com/deps.rete | head -c 4 ; echo
#  RETE
curl -sD - -o /dev/null -r 0-1023 https://my-bucket.s3.amazonaws.com/deps.rete | grep -i '206\|content-range'
#  HTTP/2 206
#  content-range: bytes 0-1023/1610
```

That `206` is the contract: a host that ignores `Range` and returns `200` is
rejected by the client (it would otherwise read the wrong bytes). For large
files, this is the whole point — a point query pulls a handful of kilobytes from a
multi-megabyte file instead of the entire thing.

## 5. In the browser

The same file serves a zero-backend web app: the [browser engine](browser.md)
range-fetches the overview first, then runs SPARQL client-side. A security
dashboard could load `deps.rete` straight from S3 and let users explore impact
with no API to build, deploy, or secure.

---

**What just happened:** one static file replaced a graph database + query API. The
data is plain RDF, the queries are standard SPARQL, and the transport is ordinary
HTTP range requests — so it works on infrastructure you already have.
