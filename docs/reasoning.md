# Reasoning & Coherence Checking

Rete provides **two complementary ways** to reason over an ontology (like a taxonomy or schema):

1. **Query Rewriting (OWL 2 QL):** Expands your query so it finds entailed answers on the fly, without touching or bloating the raw data. **Best for:** Answering queries under an ontology dynamically (even over remote files). [Jump to Query Rewriting](#reasoning-by-query-rewriting-owl-2-ql)
2. **Materialization (OWL RL / RDFS):** Computes all logical consequences (entailments) upfront and saves them into the file, or checks the data for logical contradictions. **Best for:** Baking inferences into a dataset or detecting errors.

---

## 1. Materialization & Coherence Checking (OWL RL / RDFS)

The `rete reason` command runs a forward-chaining reasoner over your default graph. It generates all obvious entailments until there are no new facts to discover, and then scans the graph for **incoherent points** (logical contradictions, such as an entity belonging to two disjoint classes).

> [!NOTE]
> **What is OWL RL?**
> OWL lets you state rules about your data (like class hierarchies, disjointness). A *reasoner* draws logical conclusions ("If A causes B, and causation is transitive, then A causes C"). OWL RL is the specific rule-based profile designed for fast, forward-chaining over triples.

> [!TIP]
> **Loading Ontologies**
> `rete build` accepts **Turtle** (`.ttl`) and **RDF/XML** (`.rdf` / `.owl`) out of the box! You can build an ontology straight into a `.rete` file without conversion.

<figure class="fig-right">
  <img src="img/reasoning.svg" alt="Asserted triples plus a subClassOf+type rule produce an inferred type triple (dashed); a disjointWith axiom plus conflicting types produce a red 'incoherent' flag.">
  <figcaption>Forward-chaining materializes entailed triples (dashed); disjointness and domain clashes surface as <strong>incoherent points</strong>.</figcaption>
</figure>

### Usage

```sh
# Report total entailments and check for contradictions
rete reason data.rete                       

# Also print the combined base + inferred graph (N-Quads)
rete reason data.rete --materialize          

# Same as above, but format as Turtle
rete reason data.rete --materialize --format ttl

# Check a REMOTE file over HTTP (lazy range reads)
rete reason --url https://host/data.rete     

# Get a terse one-line verdict (perfect for CI/CD gates)
rete reason data.rete --check                

# Re-verify a build-time coherence stamp
rete reason data.rete --verify-card          
```

If `rete reason` finds **any** inconsistency, it exits with a non-zero status, making it a perfect CI pipeline check!

### Build-Time Materialization

If you want to bake the inferred facts into your `.rete` file forever, use `--materialize` during build:

```sh
rete build data.nt -o data.rete --materialize
```
This stores inferred triples alongside asserted ones. **Trade-off:** The file becomes larger and needs rebuilding if the ontology changes, but queries run instantly without needing reasoning enabled. If the graph is incoherent, the build will **abort**.

### Build-Time Coherence Stamping

You can run a check during build and "stamp" the result into the Dataset Card. This allows readers to verify coherence **instantly** without downloading the graph:

```sh
rete build data.nt -o data.rete --reason
rete card data.rete            # View the card
```

Unlike `--materialize`, this will **not abort** on an incoherent graph—it honestly records `coherent: false` so you can still publish the status.

Verify a stamp hasn't drifted using:
```sh
rete reason data.rete --verify-card
```

### Supported Entailment Rules (Materialization)

| Rule | If the graph contains… | …then entail |
|------|------------------------|--------------|
| **subClassOf transitivity** | `c rdfs:subClassOf d` · `d rdfs:subClassOf e` | `c rdfs:subClassOf e` |
| **type propagation** | `x a c` · `c rdfs:subClassOf d` | `x a d` |
| **subPropertyOf** | `p rdfs:subPropertyOf q` · `x p y` | `x q y` |
| **subPropertyOf transitivity** | `p rdfs:subPropertyOf q` · `q rdfs:subPropertyOf r` | `p rdfs:subPropertyOf r` |
| **domain** | `p rdfs:domain c` · `x p y` | `x a c` |
| **range** | `p rdfs:range c` · `x p y` | `y a c` |
| **inverseOf** | `p owl:inverseOf q` · `x p y` | `y q x` (and reverse) |
| **SymmetricProperty** | `p a owl:SymmetricProperty` · `x p y` | `y p x` |
| **TransitiveProperty** | `p a owl:TransitiveProperty` · `x p y` · `y p z` | `x p z` |

### Inconsistency Detection (Incoherent Points)

| Kind | Triggered by |
|--------|--------------|
| `disjoint-classes` | `x a c` · `x a d` · `c owl:disjointWith d` |
| `sameas-differentfrom` | `x owl:sameAs y` · `x owl:differentFrom y` |
| `functional-property` | `p a owl:FunctionalProperty` · `x p y` · `x p z` · `y ≠ z` |
| `owl-nothing` | `x a owl:Nothing` |

### Worked Example: A Causal Model

Let's test `examples/causal.nt`, which contains both a **structural causal cycle** (`a → b → c → a`) and a **logical contradiction** (Patient `:p` is typed as both `HealthyState` and `DiseaseState`, which are disjoint).

**1. Find the cycle (Structural)**
```sh
rete sparql causal.rete "PREFIX e: <http://ex/> SELECT ?x WHERE { ?x e:causes+ ?x }"
```

**2. Find the contradiction (Logical)**
```sh
rete build examples/causal.nt -o causal.rete
rete reason causal.rete
# Output:
# [disjoint-classes] <http://ex/p> is typed as both <http://ex/DiseaseState> and <http://ex/HealthyState>
# Error: 1 inconsistency(ies) — graph is incoherent   (exit code 1)
```

### Remote / In-Browser Coherence Checking

You can run coherence checks in the browser on remote `.rete` files using HTTP range reads. The WASM build exposes three worker-only tiers based on how deep you want to check:

| Tier | Function | Reads | Finds |
|------|----------|-------|-------|
| **0 — Schema** | `check_schema_url(url)` | Just the header & schema (~1-8 KB max) | `subClassOf` cycles & unsatisfiable classes. |
| **1 — Selective**| `reason_construct_url(url)`| Only tiles touching specific T-Box rules | Instance clashes (disjoint-classes, sameAs/differentFrom). |
| **2 — Full** | `reason_url(url)` | The entire graph (downloads heavily) | Everything `rete reason` CLI finds. |

**Tier 0** is blazing fast (reads less than 10KB!) but can only find schema-level contradictions. **Tier 1** is the sweet spot for validating instances without downloading the whole file. 

You can try this in the [Playground](playground.html#dataset=causal) under the **Coherence** tab.

---

## 2. Reasoning by Query Rewriting (OWL 2 QL)

Unlike materialization, **OWL 2 QL query rewriting** doesn't change or bloat your data. Instead, it rewrites the *query itself* to fetch entailed answers from the raw data. This is ideal for large datasets where baking inferences into the file is too expensive.

It is **strictly opt-in**. Plain queries are untouched.

```sh
rete sparql     data.rete "SELECT ?o WHERE { ?o a :Aves }" --entail
rete sparql-url https://host/data.rete "…" --entail        # Lazy, over HTTP range
```
*(In the playground, just enable the **🧠 Reason** toggle!)*

### What it can infer on the fly

| Axiom | A query for … also returns … |
|---|---|
| `rdfs:subClassOf` | `?x a C` → instances of every subclass of `C` (transitively) |
| `rdfs:subPropertyOf` | `?x P ?y` → pairs related by any subproperty of `P` |
| `rdfs:domain` / `range` | `?x a C` → subjects/objects of a property whose domain/range is `⊑ C` |
| `owl:inverseOf` | `?x P ?y` → pairs `?y Q ?x` for any `Q` inverse to `P` |
| `owl:someValuesFrom` | `?x P ?_` (or `?_ P ?x`) → every `?x` that is such an `A` |

```sparql
# Example: gbif-birds
# WITHOUT --entail: Returns nothing.
# WITH --entail: Returns real occurrences by intelligently walking up the taxonomy!
SELECT ?o WHERE { ?o a <https://w3id.org/rete/gbif/taxon/class/Aves> } LIMIT 20
```

### How it works under the hood
- **Hierarchy (`subClassOf`)**: Rewritten as a zero-overhead property path (e.g., `?x a ?c . ?c rdfs:subClassOf* C`).
- **Zero Overhead**: If a class/property has no axioms attached to it in the dataset, the query engine leaves it completely alone.

*(Note: The only unhandled edge-case is PerfectRef existential chaining, but the reasoner will never return unsound/wrong data—it simply falls back to exact matching for that specific shape).*
