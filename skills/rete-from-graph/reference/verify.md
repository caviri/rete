# Verify a `.rete` (and the engine that built it)

Two layers: **(A)** sanity-check the file you just built, and **(B)** the project's
own engine-correctness test suite (when you touched the Rust).

## A. Sanity-check a built file

The `scripts/verify_rete.sh` wrapper runs these; individually:

```bash
rete validate data/foo/foo.nt          # BEFORE build: do the inputs parse? counts or a parse error
rete info     web/foo.rete             # header: version, sections, counts
rete stats    web/foo.rete             # size, counts, graphs, pyramid, top predicates
rete verify   web/foo.rete             # content-hash check (detects corruption/truncation)
rete card     web/foo.rete             # the embedded Dataset Card (if --card)
rete sparql   web/foo.rete "SELECT ?c (COUNT(*) AS ?n) WHERE { ?s a ?c } GROUP BY ?c ORDER BY DESC(?n) LIMIT 20"
rete schema   web/foo.rete             # the effective class-to-class schema (sanity-check the model)
```

Good signs: `validate` green, `stats` triple count matches the source, `schema`
shows the classes/relations you modeled, a spot-check SPARQL returns sane rows.

Remote sanity (after upload, §rete-publish): `rete card-url <url>` and
`rete sparql-url <url> "<query>"` exercise the HTTP-range path the playground uses.

## B. Engine-correctness suite (when you change the Rust)

The project follows **SQLite-style rigor** (full design in `dev/testing.md`). The
single highest-value tool is the differential oracle.

### Differential oracle vs Oxigraph — the gold standard
`crates/bench/tests/differential.rs` runs the SAME SPARQL through **rete and
Oxigraph** over the same data and asserts the result rows agree. Oxigraph is the
SPARQL 1.1 reference, so any divergence is (almost always) a rete bug — this found
four real correctness bugs the moment it was switched on (ROUND half-rounding,
FILTER EBV collapse, aggregate-over-expression, string-function leniency).

```bash
cargo test -p bench --test differential
```
Adding a query to the battery is one line; a disagreement is a bug.

### Property / invariant tests — `crates/rete-core/tests/properties.rs`
`proptest` random graphs assert:
- `prop_roundtrip` — triples in == triples out (build→open).
- `prop_deterministic` — building the same graph twice is byte-identical.
- `prop_lazy_equals_eager` — a lazy `RangeReader` open answers identically to an
  eager in-memory open (guards the whole tiled/range-read path).
- `fuzz_arbitrary_bytes_never_panic` / `fuzz_mutated_image_never_panic` — opening
  arbitrary/bit-flipped bytes returns `Err`, never panics.

```bash
cargo test -p rete-core               # properties, ranged, robustness, roundtrip, community_eval
cargo test --workspace                # everything, incl. the CLI tests in crates/rete-cli/tests
```

### Coverage-guided fuzzing
`cargo fuzz` targets exist; `dev/coverage.sh` measures branch coverage. Run the
differential battery under coverage to drive the `sparql/expr.rs` branches unit
tests miss.

## When NOT to rebuild the WASM

If you only changed converters/data/playground JS, you do NOT rebuild the engine.
If you DID change the Rust and the playground needs it: rebuild the **regular**
no-modules WASM in `rete-dev` (NOT the asyncify image — its wasm-opt corrupts the
externref table export → boot crash). Verify with
`wasm-dis … | grep __wbindgen_externrefs` (must show `(table $1)`).
