# R API

`rete` is the R client for `.rete` files: native bindings (extendr) to the
same Rust engine behind the [CLI](cli.md), the
[Python client](python.md), and the
[browser playground](playground-guide.md). It opens a graph from a **local
path, an HTTP(S) URL, or a raw vector** and queries it with SPARQL, returning
ordinary data frames — remote files are read lazily over HTTP `Range`
requests, so a selective query over a multi-GB file fetches kilobytes, never
the file.

```r
# Straight from GitHub — needs Rust ≥ 1.87 on PATH (https://rustup.rs):
install.packages("remotes")
remotes::install_github("caviri/rete", subdir = "clients/r", build = FALSE)

# A specific branch, tag, or commit:
remotes::install_github("caviri/rete@main", subdir = "clients/r", build = FALSE)
```

One command fetches the repository, compiles the bundled Rust engine
(a few minutes the first time), and installs the package — no clone needed.
After install, `vignette("rete")` opens an offline tour that mirrors this
page, and `?rete_open`, `?rete_query`, `?rete_build` are the reference
pages.
`build = FALSE` matters: the package lives in a monorepo and its Rust crate
references the engine at the repository root, so it must install from the
full source tree rather than a pre-built subdir tarball (that also rules out
`pak::pak("caviri/rete/clients/r")` for now). Binary installs via
R-universe/CRAN (no Rust required) are planned; GitHub is the install path
today.

## Open a graph and query it

```r
library(rete)

g <- rete_open("https://data.graphplaza.com/boe/boe.rete")   # remote, lazy
g <- rete_open("data/example.rete")                          # local file, lazy too
g <- rete_open(file_image)                                   # raw vector, eager

rete_query(g, "
  SELECT ?title WHERE {
    ?law <http://data.europa.eu/eli/ontology#title> ?title
  } LIMIT 5
")
```

`rete_query()` returns what an R user expects:

- **SELECT** → a `data.frame`, one column per variable. IRI brackets are
  stripped; `xsd:integer` family literals become integers (doubles on
  overflow), `xsd:decimal`/`double`/`float` become doubles, `xsd:boolean`
  becomes logical; everything else stays character.
- **ASK** → a logical scalar.
- **CONSTRUCT / DESCRIBE** → a `data.frame` with `subject`, `predicate`,
  `object`.

`rete_query_raw()` returns the engine's JSON envelope parsed to a list, with
terms in full N-Triples token fidelity (`<iri>`, `"lit"^^<datatype>`,
`_:bnode`) — reach for it when the coercions above are too helpful.

Both opens are **lazy**: the header, dictionary directory, and index tile
directories load up front; tile payloads fault in per query and stay cached
on the handle, so repeated queries get faster. The host serving a remote file
must answer `Range` requests with `206 Partial Content` (any S3/R2/CDN/GitHub
URL does — see [Hosting your .rete](hosting.md)); anything else is a loud
error, never a silently wrong slice.

```r
rete_stats(g)
#> $fileLength  … $bytes  … $requests
```

`rete_stats()` reports the physical traffic since open — the number that
makes the lazy story visible: a selective query over a multi-hundred-MB
remote file typically fetches well under 1% of it.

## Reasoning

```r
rete_query(g, query, reason = TRUE)
```

`reason = TRUE` answers with OWL 2 QL entailment, computed by query rewriting
over the ontology embedded in the file — no materialization, so it works on
remote files too. See [Reasoning](reasoning.md).

## Explore a file you did not build

```r
rete_info(g)            # quads, terms, pyramid levels, named graphs
rete_card(g)            # the embedded Dataset Card as a list (or NULL)
rete_examples(g)        # starter queries the card carries, as a data.frame
rete_schema(g)          # class + predicate profile, two data.frames
rete_prefix_search(g, "Mad")        # label autocomplete
rete_text_search(g, "madrid ley")   # full-text (needs a text-indexed file)
rete_content_hash(g)    # blake3-16 hex
```

`rete_card()` and `rete_examples()` fetch only the metadata section's byte
range on lazy opens — reading a remote file's card costs a few requests.
Every `sparql` entry in `rete_examples()` runs as-is:

```r
ex <- rete_examples(g)
rete_query(g, ex$sparql[[1]])
```

## Build a .rete from R

```r
nt <- '
<urn:x:alice> <http://xmlns.com/foaf/0.1/knows> <urn:x:bob> .
<urn:x:alice> <http://xmlns.com/foaf/0.1/name> "Alice" .
'
img <- rete_build(nt,
  format = "nt",                       # nt | nq | ttl | rdfxml
  card = list(
    title = "Tiny demo",
    description = "Two triples about Alice",
    license = "CC0-1.0"
  ),
  pyramid = "louvain",                 # louvain | types | none
  text_index = TRUE
)
writeBin(img, "demo.rete")             # or query it in place:
rete_query(rete_open(img), "SELECT ?n WHERE { ?s <http://xmlns.com/foaf/0.1/name> ?n }")
```

Counts (`triple_count`, `term_count`, …) are stamped into the card
automatically. Pass `derive_card = TRUE` and the card also carries the
**auto-derived profile** — predicates, classes, vocabularies, datatypes,
languages, hubs, signals and the tiered starter-query library — from the same
code `rete build --card` runs, so the same graph yields a byte-identical card.
It is opt-in: derivation walks the graph twice more, and the default keeps
writing exactly the bytes it always did.

In-memory assembly suits tests and small graphs; for large datasets use the
[`rete build` CLI](cli.md), which streams and compresses.

## The same file everywhere

A `.rete` built anywhere is readable everywhere: this client, the
[Python client](python.md), the [JavaScript client](javascript.md), the
[CLI](cli.md), and the [playground](playground-guide.md) all read the same
bytes over the same range-read discipline — publish one file on any static
host and every runtime gets it lazily.
