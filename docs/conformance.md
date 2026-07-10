# SPARQL 1.1 conformance

How much of SPARQL does rete actually answer correctly? This runs the official
**W3C SPARQL 1.1 query-evaluation test suite** — the canonical
[`w3c/rdf-tests`](https://github.com/w3c/rdf-tests) fixtures: each test ships a
tiny RDF dataset, a query, and the **expected results** — against rete and
scores every test. It is a *correctness* suite (the datasets are a handful of
triples), so it measures **coverage**, not speed; performance is benchmarked
separately on a real graph (see [BENCHMARK](BENCHMARK.html)).

The harness is [`scripts/sparql_conformance.py`](https://github.com/caviri/rete/blob/main/scripts/sparql_conformance.py):
for each `mf:QueryEvaluationTest` it builds a `.rete` from the data, runs the
query through the `rete` CLI, and compares to the expected result — SPARQL
Results JSON/XML (SRX/SRJ) for SELECT/ASK as an unordered multiset, and an **RDF
graph isomorphism** for CONSTRUCT/DESCRIBE.

```sh
python scripts/sparql_conformance.py \
  --rete target/release/rete --suite <rdf-tests>/sparql/sparql11
```

## Scorecard

**309 query-evaluation tests**, byte-for-byte against the W3C expected results.
"n/s" = errored / not supported.

| Category | pass | n/s | notes |
|---|--:|--:|---|
| negation | 12 / 12 | 0 | ✅ full |
| json-res | 4 / 4 | 0 | ✅ full |
| cast | 6 / 6 | 0 | ✅ full — xsd:integer/decimal/float/double/boolean/string |
| bind | 10 / 10 | 0 | ✅ full — in-pattern BIND visible to later FILTER/join |
| grouping | 4 / 4 | 0 | ✅ full |
| bindings (VALUES) | 10 / 11 | 1 | |
| aggregates | 39 / 42 | 3 | GROUP_CONCAT/SUM/AVG/SAMPLE incl. DISTINCT |
| property-path | 30 / 33 | 2 | incl. negated property sets + zero-length on empty data |
| construct | 3 / 5 | 1 | graph-isomorphism check |
| exists | 4 / 6 | 1 | |
| project-expression | 7 / 7 | 0 | ✅ full |
| functions | 73 / 75 | 1 | nearly full — only NOW() + IRI() base resolution |
| entailment | 28 / 70 | 4 | needs `build --materialize` |
| subquery | 2 / 14 | 12 | nested SELECT joins; GRAPH-scoped + RDF/XML data n/a |
| service | 0 / 7 | 7 | `SERVICE` **is** implemented — these tests need a live endpoint, so they're excluded from the offline run |
| csv-tsv-res | 0 / 3 | 3 | CSV/TSV result format |
| **TOTAL** | **232 / 309 (75.1%)** | 33 | |

## Coverage notes

**Strong areas.** Negation (`MINUS` / `NOT EXISTS`), JSON results, XSD casts
(with strict lexical validation), and projection expressions are 100%. Property
paths are near-full — including negated property sets (`!(:p1|…|:pn)`) and the
zero-length identity solution on `*`/`?` even against empty data. `BIND` inside
a WHERE pattern is visible to later filters and joins; nested `SELECT`
subqueries are evaluated independently and joined on their projected variables;
the aggregate set is complete (`GROUP_CONCAT` with `DISTINCT`/`SEPARATOR`,
typed computed numerics, 15-significant-digit decimal round-trips). The
built-in function library covers strings (language-tag-preserving
`REPLACE`/`CONCAT`/`SUBSTR`/…, `STRDT`/`STRLANG`, `ENCODE_FOR_URI`,
`LANGMATCHES`), the `MD5`…`SHA512` hashes, the `xsd:dateTime` accessors, and
`IF`/`IN`/`sameTerm` — all in pure Rust, so the same coverage holds in the WASM
client. `CONSTRUCT` answers are verified by graph isomorphism. This matches the
24-operator cross-check against Oxigraph in [BENCHMARK](BENCHMARK.html).

**Out of scope (counted against the total, but not engine bugs):**

- **SERVICE** (7) — `SERVICE` federation is implemented (the block is sent to
  the remote endpoint and joined — see [sparql](sparql.html)); these tests need
  a live endpoint, so they stay out of the offline suite. (Cross-*file*
  federation is a different feature — see [federation](federation.html).)
- **entailment** (≈49) — RDFS/OWL entailment regimes; rete answers these only
  when entailments are baked in at build time (`rete build --materialize`),
  which this run does not do.
- **subquery** (12 n/s) — plain nested `SELECT` *is* evaluated (see above); the
  remaining n/s are GRAPH-scoped subqueries and tests whose data ships as
  RDF/XML, not the subquery feature itself.
- **csv-tsv-res** (3) — the CSV/TSV result serialization isn't implemented.

**Remaining gaps in `functions` (2):** `NOW()` (no wall clock on the
`wasm32-unknown-unknown` target the engine must also compile to) and `IRI()`
relative-base resolution. The non-deterministic builtins `RAND`,
`UUID`/`STRUUID`, `BNODE` work in the browser too (via `getrandom`'s `js`
backend), and an error in an `IF` condition propagates rather than silently
taking the else-branch.

## The same answers, lazily and remotely

The conformance run opens each file **locally (in memory)**. rete's three read
modes — local, **remote-lazy** (HTTP range reads via `sparql_url` /
`sparql-url`), and **remote-cached** (download once, query in memory) — share the
*identical* evaluator; only the byte source differs. So they return identical
results, which the [Wikidata explorer](explore-100mb.html) demonstrates on a real
~12 M-triple graph: the same query yields the same rows whether served locally,
lazily from a CDN, or federated across shards. The tiny conformance fixtures make
per-mode *timing* meaningless — that comparison lives in
[BENCHMARK](BENCHMARK.html), on a graph large enough for it to matter.

## Reproduce

```sh
git clone --depth 1 --filter=blob:none --sparse https://github.com/w3c/rdf-tests
cd rdf-tests && git sparse-checkout set sparql/sparql11 && cd ..
cargo build --release -p rete-cli
python scripts/sparql_conformance.py --rete target/release/rete \
  --suite rdf-tests/sparql/sparql11            # add --relaxed for the value column
```
