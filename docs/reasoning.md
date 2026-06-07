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

## Usage

```sh
rete reason data.rete                       # report entailment count + incoherent points
rete reason data.rete --materialize          # also print base + inferred graph (N-Quads)
rete reason data.rete --materialize --format ttl
```

`rete reason` **exits non-zero when any inconsistency is found**, and zero when
the graph is coherent — so it drops straight into CI as a coherence check.

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
