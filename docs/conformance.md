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
| bindings (VALUES) | 9 / 11 | 2 | |
| property-path | 21 / 33 | 7 | strong |
| construct | 3 / 5 | 1 | graph-isomorphism check |
| exists | 4 / 6 | 1 | |
| project-expression | 6 / 7 | 0 | |
| aggregates | 27 / 42 | 4 | |
| functions | 64 / 75 | 7 | string/cast/hash/datetime + IF/IN/sameTerm |
| entailment | 28 / 70 | 4 | needs `build --materialize` |
| subquery | 1 / 14 | 12 | not supported |
| service | 0 / 7 | 7 | SPARQL federation (N/A) |
| csv-tsv-res | 0 / 3 | 3 | CSV/TSV result format |
| **TOTAL** | **199 / 309 (64.4%)** | 48 | |

## Findings

0. **In-pattern `BIND` scoping (fixed — +10 tests, → 64.4%).** A `BIND(expr AS
   ?v)` written *inside* a WHERE pattern is now an in-tree plan node, so a
   *following* `FILTER` or join sees the bound variable (previously the BIND was
   deferred to projection time, after filtering — so `{ ?s :v ?o BIND(?o+1 AS
   ?z) FILTER(?z=3) }` wrongly returned nothing). Top-level projection aliases
   (`SELECT (expr AS ?v)`) still apply after aggregation, unchanged. This took
   `bind` and `grouping` to 100% and lifted four entailment BIND tests.

1. **Built-in function coverage (fixed — +52 tests).** Two pushes lifted strict
   conformance from **34.6% → 61.2%**:
   - **Computed numerics are typed.** Arithmetic / aggregates / numeric
     functions evaluate to the right *value*; `sparql::fmt_num_typed` tags the
     result `xsd:integer` (whole) or `xsd:decimal` (fractional) so the
     serializer emits the datatype (34.6% → 44.3%).
   - **The string/cast/hash/datetime built-ins now exist, and return proper
     terms** (44.3% → 61.2%). `functions` went 19 → 64 and `cast` 0 → 6:
     - **Strings:** `STRBEFORE`/`STRAFTER` (with the SPARQL argument-compatibility
       and language-tag rules), `REPLACE`, `CONCAT`, `UCASE`/`LCASE`/`SUBSTR`
       now preserve the language tag and emit a real literal term (`"FOO"@en`)
       rather than bare text; `STRDT`/`STRLANG`, `IRI`/`URI`, `ENCODE_FOR_URI`,
       `LANGMATCHES`.
     - **Hashes:** `MD5`, `SHA1`, `SHA256`, `SHA384`, `SHA512` (pure-Rust
       RustCrypto, so they compile to wasm too).
     - **Date/time:** `YEAR`/`MONTH`/`DAY`/`HOURS`/`MINUTES`/`SECONDS`/`TZ`/
       `TIMEZONE` over `xsd:dateTime`.
     - **XSD casts:** `xsd:integer/decimal/float/double/boolean/string(...)`,
       with strict lexical validation (a non-conforming string is a type error)
       and canonicalization on cast-to-string.
     - **Expression forms:** `IF`, `IN`/`NOT IN`, `sameTerm`.

2. **Genuinely strong areas:** negation (`MINUS` / `NOT EXISTS`), JSON results,
   and the XSD casts are 100%; property paths, VALUES, GROUP BY, and CONSTRUCT
   (verified by graph isomorphism) are solid. This matches the 24-operator
   cross-check against Oxigraph in [BENCHMARK](BENCHMARK.html).

3. **Out of scope (counted, but not engine bugs):**
   - **SERVICE** (7) — SPARQL federation to a remote endpoint; rete is a file,
     not an endpoint. (Cross-*file* federation is a different feature — see
     [federation](federation.html).)
   - **entailment** (≈49) — RDFS/OWL entailment regimes; rete answers these only
     when entailments are baked in at build time (`rete build --materialize`),
     which this run does not do.
   - **subquery** (≈12) — nested `SELECT` is rejected rather than evaluated.
   - **csv-tsv-res** (3) — the CSV/TSV result serialization isn't implemented.

4. **What's left in `functions` (11):** the non-deterministic builtins
   (`NOW`, `RAND`, `UUID`/`STRUUID`, `BNODE`), `IRI()` relative-base resolution,
   and `IF`-error-propagation edge cases — honest "not yet" rather than silent
   bugs (an unimplemented function errors rather than returning a wrong answer).

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
