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
| service | 0 / 7 | 7 | SPARQL federation (N/A) |
| csv-tsv-res | 0 / 3 | 3 | CSV/TSV result format |
| **TOTAL** | **232 / 309 (75.1%)** | 33 | |

## Findings

- **Boolean-valued projections + strict arithmetic (fixed — +3 tests).** A
  comparison or logical expression in value position — `(?y = ?z AS ?eq)` —
  yields a typed `xsd:boolean`, and arithmetic (`+ - * /`) now requires
  numeric-typed operands (a `"1"^^xsd:string` is a type error, not 1), matching
  the RDF 1.1 "corrected" expectations. `project-expression` is now 100%.

- **Property paths — negated sets + zero-length on empty data (fixed — +9
  tests, → 71.8%).** `!(:p1|…|:pn)` (and its inverse members, which `spargebra`
  wraps in a `Reverse`) is now a `PathAst::NegatedSet` evaluated as one step over
  any predicate not in the set. And a zero-length-capable path (`*`/`?`) with a
  constant endpoint that isn't in the graph now yields the identity solution
  (`:x :p* ?o` ⇒ `?o = :x`, even on an empty dataset). `property-path` 21 → 30.

- **Subqueries + aggregate completeness (fixed — +9 tests, → 68.9%).** A nested
  `SELECT` is now lowered to an in-tree `Plan::Subquery`, evaluated independently
  to its projected solutions, which then join with the surrounding pattern on
  shared variables (so `{ {SELECT (GROUP_CONCAT(?o) AS ?g)…} FILTER(?g=…) }`
  works). On the back of that, the aggregate set was finished: `GROUP_CONCAT`
  returns a real simple literal and honours `DISTINCT`/`SEPARATOR`, `AVG` of an
  empty group is `0` (and a non-numeric value is a type error), and computed
  decimals round-trip through 15 significant digits so a sum like `11.1` no
  longer serializes as `11.100000000000001`. `aggregates` 27 → 39, `grouping`
  100%. Remaining subquery gaps are GRAPH-scoped subqueries and tests whose data
  ships as RDF/XML.

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

4. **What's left in `functions` (2):** `NOW()` (no wall clock on the
   `wasm32-unknown-unknown` target the engine must also compile to) and `IRI()`
   relative-base resolution. The non-deterministic builtins `RAND`,
   `UUID`/`STRUUID`, `BNODE` are implemented on `getrandom` (browser backend via
   its `js` feature, so they work in the WASM client too), and `IF` now
   propagates an error in its condition rather than silently taking the
   else-branch.

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
