# Reasoning & coherence checking

`rete reason` runs a **prototype forward-chaining OWL RL / RDFS reasoner** over a
file's default graph: it materializes the obvious RDFS/OWL entailments to a
fixpoint, then scans the closed graph for **incoherent points** — logical
contradictions such as an individual that belongs to two disjoint classes.

> **OWL RL in one paragraph.** OWL (the Web Ontology Language) lets you state
> *schema* axioms about your data — class hierarchies, property characteristics,
> disjointness, identity. A *reasoner* draws the logical consequences (e.g. "if
> `:a causes :b` and causation is transitive, then `:a causes :c`") and detects
> contradictions. OWL RL is the rule-based profile of OWL designed exactly for
> forward-chaining materialization over triples, which is what this implements.

> **Scope — this is a prototype subset, NOT full OWL DL.** It covers the rules in
> the tables below by exact-string matching on canonical N-Triples tokens. It does
> not do equality reasoning beyond the checks listed, class expressions
> (`owl:Restriction`, intersections/unions), cardinality, datatype reasoning, or
> open-world consistency in the OWL DL sense. Use it as a lightweight coherence
> gate, not a certified entailment regime.

<figure class="fig-right">
  <img src="img/reasoning.svg" alt="Asserted triples plus a subClassOf+type rule produce an inferred type triple (dashed); a disjointWith axiom plus conflicting types produce a red 'incoherent' flag.">
  <figcaption>Forward-chaining materializes entailed triples (dashed); disjointness and domain clashes surface as <strong>incoherent points</strong>.</figcaption>
</figure>

## Usage

```sh
rete reason data.rete                       # report entailment count + incoherent points
rete reason data.rete --materialize          # also print base + inferred graph (N-Quads)
rete reason data.rete --materialize --format ttl
```

`rete reason` **exits non-zero when any inconsistency is found**, and zero when
the graph is coherent — so it drops straight into CI as a coherence check.

## Build-time materialization

`rete reason --materialize` *prints* the closed graph but doesn't change the file.
To **persist** the entailments — so they ship in the `.rete` and need no
query-time reasoning — materialize at build time:

```sh
rete build data.nt -o data.rete --materialize
```

This runs the same reasoner over the default graph during the build and stores the
inferred triples alongside the asserted ones (the index deduplicates any overlap).
After that, a plain triple/SPARQL query sees the entailed triples directly — e.g.
with `Class subClassOf Super` and `x a Class` asserted, `x a Super` is queryable
with no reasoner in the loop. The build **aborts** (non-zero) if the graph is
logically incoherent, so it never bakes a contradiction into a published file.

Trade-off: a materialized file is larger and fixes the entailments at build time
(re-build to re-materialize after the ontology changes); an un-materialized file
stays compact and you run `rete reason` on demand.

## Entailment rules (materialized to a fixpoint)

| Rule | If the graph contains… | …then entail |
|------|------------------------|--------------|
| subClassOf transitivity | `c rdfs:subClassOf d` · `d rdfs:subClassOf e` | `c rdfs:subClassOf e` |
| type propagation | `x a c` · `c rdfs:subClassOf d` | `x a d` |
| subPropertyOf | `p rdfs:subPropertyOf q` · `x p y` | `x q y` |
| subPropertyOf transitivity | `p rdfs:subPropertyOf q` · `q rdfs:subPropertyOf r` | `p rdfs:subPropertyOf r` |
| domain | `p rdfs:domain c` · `x p y` | `x a c` |
| range | `p rdfs:range c` · `x p y` | `y a c` |
| inverseOf | `p owl:inverseOf q` · `x p y` | `y q x` (and the reverse direction) |
| SymmetricProperty | `p a owl:SymmetricProperty` · `x p y` | `y p x` |
| TransitiveProperty | `p a owl:TransitiveProperty` · `x p y` · `y p z` | `x p z` |

## Inconsistency rules (the incoherent points)

Detection runs **after** materialization, so entailed types count — a
disjointness violation that only shows up once `rdfs:subClassOf` has propagated is
still caught.

| `kind` | Triggered by |
|--------|--------------|
| `disjoint-classes` | `x a c` · `x a d` · `c owl:disjointWith d` (either direction) |
| `sameas-differentfrom` | `x owl:sameAs y` · `x owl:differentFrom y` (either direction) |
| `functional-property` | `p a owl:FunctionalProperty` · `x p y` · `x p z` · `y ≠ z` (and not `y owl:sameAs z`) |
| `owl-nothing` | `x a owl:Nothing` |

## Worked example: a causal model

`examples/causal.nt` is a small causal graph. It carries (1) a causal **cycle**
`a → b → c → a`, and (2) an **incoherent point**: a patient `:p` typed as both
`HealthyState` and `DiseaseState`, which are declared `owl:disjointWith`.

The two kinds of problem are found by two complementary tools:

**A causal cycle** is a *structural* fact about the data — find it with a SPARQL
property path:

```sh
rete sparql causal.rete "PREFIX e: <http://ex/> SELECT ?x WHERE { ?x e:causes+ ?x }"
#  ?x=<http://ex/a>
#  ?x=<http://ex/b>
#  ?x=<http://ex/c>
```

**A disjointness (or functional-property) violation** is a *logical*
contradiction — find it with `rete reason`:

```sh
rete build examples/causal.nt -o causal.rete
rete reason causal.rete
#  inferred 9 new triple(s)
#  1 inconsistency(ies) found:
#    [disjoint-classes] <http://ex/p> is typed as both <http://ex/DiseaseState> and <http://ex/HealthyState>, which are owl:disjointWith
#  Error: 1 inconsistency(ies) — graph is incoherent   (exit code 1)
```

The entailment count (9) reflects the materialized `subClassOf` chain
(`Disease ⊑ Condition ⊑ Factor`) and the transitive `:causes` closure
(`a causes c`, etc.). Because the graph is incoherent, the command exits non-zero.

## Remote / in-browser coherence

The same coherence checks run **in the browser against a remote `.rete` over HTTP
range reads** — drop a file on any range-serving host (S3, GitHub Pages, a CORS
proxy), hand the client a URL, and check coherence with no server and no full
download. The WASM build exposes three entry points, tiered by how much of the
file they touch. They mirror the SHACL family (`shacl_url` / `shacl_construct_url`)
and are **worker-only** (the engine is synchronous and uses blocking XHR, which
browsers permit only off the main thread). Each returns a JSON envelope with a
`remote: { fileLength, bytes, requests }` block so the UI can show exactly how
little of the file was pulled; a failed range fetch mid-check is an **error**,
never a silently-incomplete (and so possibly false-"coherent") result.

| Tier | Function | Reads | Finds |
|------|----------|-------|-------|
| **0 — schema** | `check_schema_url(url)` | header + dictionary + pyramid-meta only (~2–3 ranges, **O(1) in graph size**, never the triple index) | subClassOf cycles; **unsatisfiable classes** (a class that is a subclass of two `owl:disjointWith` classes) |
| **1 — selective** | `reason_construct_url(url, construct)` | only the tiles the CONSTRUCT's constant-predicate patterns touch (one warm cache) | every contradiction visible from `rdf:type` + the class/equality T-Box (disjoint-class clashes, `sameAs`/`differentFrom`) |
| **2 — full** | `reason_url(url[, graph])` | materializes the whole graph (≈ the entire file) | every incoherent point the CLI `rete reason` finds |

**Tier 0** is the flagship: it answers *"is this ontology coherent?"* for a
multi-GB graph by reading tens of kilobytes, because the `subClassOf` DAG plus the
`owl:disjointWith` / `owl:equivalentClass` axioms travel **index-free** in the
schema pyramid (the same section `rete summary` reads). It cannot see
instance-level clashes (a node typed into disjoint classes, functional-property
clashes) — those need the A-Box, i.e. Tier 1 or 2. Soundness is bounded by what the
pyramid ships: the hierarchy is capped, so on a very large ontology a pruned
ancestor can hide an unsatisfiable class (a false *coherent*, never a false
*incoherent*).

**Tier 1** is the selective sweet spot. The default slice is a `UNION` of
constant-predicate `CONSTRUCT` branches so each routes to a single predicate's
tiles:

```sparql
CONSTRUCT { ?x rdf:type ?c . ?sub rdfs:subClassOf ?sup .
            ?c1 owl:disjointWith ?c2 . ?s1 owl:sameAs ?s2 . ?f1 owl:differentFrom ?f2 }
WHERE { { ?x rdf:type ?c } UNION { ?sub rdfs:subClassOf ?sup }
        UNION { ?c1 owl:disjointWith ?c2 } UNION { ?s1 owl:sameAs ?s2 }
        UNION { ?f1 owl:differentFrom ?f2 } }
```

> **Slice-correctness invariant.** A Tier-1 CONSTRUCT **must** pull the T-Box
> predicates (`rdfs:subClassOf`, `owl:disjointWith`, `owl:sameAs`) into the slice,
> not just `rdf:type`. The reasoner detects a disjoint-class clash only *after*
> `subClassOf` type-propagation, so a slice missing `subClassOf` would silently
> miss a propagation-dependent contradiction.

### Try it

The [100 MB explorer](explore-100mb.html) has a **Coherence** task with all three
tiers. Because the wikidata graph carries no OWL axioms, the tasks run against a
tiny same-origin causal ontology (`examples/causal.rete`) with both kinds of
defect planted: Tier 0 reports `:Relapsed` **unsatisfiable** (a subclass of the
disjoint `HealthyState` and `DiseaseState`) from ~2–3 ranges; Tier 1/2 report the
**instance** clash (`:p` is both). The live counter shows how few bytes each tier
fetches.
