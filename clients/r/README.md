# rete for R

Query local and remote [`.rete`](https://caviri.github.io/rete/) RDF graph
files with SPARQL, getting data frames back. Remote files are read **lazily
over HTTP range requests** — a selective query against a multi-gigabyte graph
on plain object storage fetches kilobytes, with no database server anywhere.

The package binds the same Rust engine that powers the
[rete CLI](https://caviri.github.io/rete/cli.html), the
[Python client](https://pypi.org/project/rete-graph/), the
[JavaScript client](https://www.npmjs.com/package/rete-graph), and the
[browser playground](https://caviri.github.io/rete/playground.html) — one
file format, identical results everywhere.

## Install

```r
# Needs Rust >= 1.87 on PATH (https://rustup.rs) to compile the engine:
install.packages("remotes")
remotes::install_github("caviri/rete", subdir = "clients/r", build = FALSE)
```

`build = FALSE` is required: the package lives in a monorepo and its Rust
crate references the engine at the repository root, so it must install from
the full source tree. Binary installs (no Rust needed) via R-universe/CRAN
are planned.

## Quick start

```r
library(rete)

g <- rete_open("https://data.graphplaza.com/boe/boe.rete")
g
#> <rete graph> https://data.graphplaza.com/boe/boe.rete
#>   465,543 quads, 111,203 terms, 3 pyramid level(s), 1 named graph(s)

rete_query(g, "SELECT ?s ?p ?o WHERE { ?s ?p ?o } LIMIT 3")   # a data.frame

rete_stats(g)          # how little was actually fetched
rete_card(g)           # the file's embedded Dataset Card
rete_examples(g)       # starter queries the file carries — run any as-is
rete_schema(g)         # class + predicate profile
```

Build a `.rete` from RDF text, query it in place, or write it out:

```r
img <- rete_build('<urn:a> <urn:knows> <urn:b> .',
                  card = list(title = "Tiny demo"))
rete_query(rete_open(img), "SELECT ?o WHERE { <urn:a> <urn:knows> ?o }")
writeBin(img, "demo.rete")
```

## Documentation

- [R API guide](https://caviri.github.io/rete/r.html) — the full tour
- `vignette("rete")` — the same, offline, after install
- `?rete_open`, `?rete_query`, `?rete_build` — reference pages
- [Maintainer notes](https://caviri.github.io/rete/clients-dev.html) —
  building, testing (Docker), and the CRAN/R-universe release path

## License

Apache-2.0. The compiled package statically links the `rete-core` engine
from this repository.
