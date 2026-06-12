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
| bindings (VALUES) | 9 / 11 | 2 | |
| property-path | 21 / 33 | 7 | strong |
| grouping | 3 / 4 | 0 | |
| construct | 3 / 5 | 1 | graph-isomorphism check |
| exists | 4 / 6 | 1 | |
| project-expression | 6 / 7 | 0 | |
| aggregates | 27 / 42 | 5 | |
| functions | 19 / 75 | 45 | many builtins missing |
| bind | 5 / 10 | 0 | |
| entailment | 24 / 70 | 4 | needs `build --materialize` |
| subquery | 0 / 14 | 13 | not supported |
| cast | 0 / 6 | 6 | xsd cast fns missing |
| service | 0 / 7 | 7 | SPARQL federation (N/A) |
| csv-tsv-res | 0 / 3 | 3 | CSV/TSV result format |
| **TOTAL** | **137 / 309 (44.3%)** | 94 | |

## Findings

1. **Computed numerics are now typed (fixed — +30 tests).** rete used to
   evaluate arithmetic / aggregates / numeric functions to the right *value* but
   serialize it as a plain literal (`"11"`) instead of `"11"^^xsd:integer`,
   strictly failing ~30 otherwise-correct tests. `sparql::fmt_num_typed` now
   tags computed numerics with `xsd:integer` (whole) or `xsd:decimal`
   (fractional) so the result serializer emits the datatype — lifting strict
   conformance **34.6% → 44.3%** (aggregates 12→27, functions 13→19, bind 2→5,
   project-expression 3→6) with no new operators. (A few `xsd:double` cases
   still differ — exact promotion needs operand-type tracking we don't carry
   through `f64`.)

2. **Genuinely strong areas:** negation (`MINUS` / `NOT EXISTS`) and the JSON
   results format are 100%; property paths (21/26), VALUES, GROUP BY, and
   CONSTRUCT (verified by graph isomorphism) are solid. This matches the
   24-operator cross-check against Oxigraph in [BENCHMARK](BENCHMARK.html).

3. **Out of scope (counted, but not engine bugs):**
   - **SERVICE** (7) — SPARQL federation to a remote endpoint; rete is a file,
     not an endpoint. (Cross-*file* federation is a different feature — see
     [federation](federation.html).)
   - **entailment** (≈49) — RDFS/OWL entailment regimes; rete answers these only
     when entailments are baked in at build time (`rete build --materialize`),
     which this run does not do.
   - **subquery** (14) — nested `SELECT` is rejected rather than evaluated.
   - **cast** (6), **csv-tsv-res** (3) — xsd cast functions and the CSV/TSV
     result serialization aren't implemented.

4. **Missing builtins** account for most of the `functions` shortfall (45 n/s) —
   each unimplemented function (string, hash, datetime, etc.) errors rather than
   returning a wrong answer, so they're honest "not yet" rather than silent bugs.

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
