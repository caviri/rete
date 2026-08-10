# Semantic Zoom: The Schema Pyramid

**The core idea:** When you open an unfamiliar `.rete` file, you immediately want to know, "What is in here?" The **Schema Pyramid** acts as a map legend with a built-in zoom control. 

At a glance, you see the broad categories (e.g., `Agent: 4k`, `Place: 8k`). Zoom in, and `Agent` resolves into `Person` and `Organisation`. Zoom further, and `Person` becomes `Scientist` and `Artist`. This entire leveled legend is shipped *inside* the file and can be fetched remotely in just 2 or 3 tiny HTTP range requests—**without ever touching the massive triple index.**

## How It Works

The schema pyramid is automatically generated during `rete build` whenever your data contains an `rdf:type` hierarchy (using `rdfs:subClassOf`). 

1.  **The Hierarchy:** The data's `subClassOf` axioms define an abstraction tree (`Astronomer ⊑ Scientist ⊑ Person ⊑ Agent`).
2.  **The Rollup:** Rete rolls every instance's type up that hierarchy at various depths, storing one distinct type histogram per level.
3.  **The Storage:** These histograms live in the **pyramid-meta** section. Range-reading clients can fetch this instantly.

## Building the Pyramid

If your dataset already has typed instances (`a`) and a class hierarchy (`rdfs:subClassOf`), building the pyramid requires zero extra flags:

```sh
rete build data.nt -o data.rete
```

*Note: If your hierarchy is implied but not explicitly stated, you can use the `rete build --materialize` flag to infer it via the RDFS/OWL-RL reasoner.*

## Using the Pyramid

### Command Line Interface

Use `rete summary` to view the entire pyramid at once. Read from top (Level 0, the most abstract) to bottom (the leaves):

```text
$ rete summary people.rete
schema pyramid — 4 level(s), 6 class(es):
  level 0 (depth 0): <http://ex/Agent>×4
  level 1 (depth 1): <http://ex/Person>×3, <http://ex/Organisation>×1
  level 2 (depth 2): <http://ex/Scientist>×2, <http://ex/Artist>×1, <http://ex/Organisation>×1
  level 3 (depth 3): <http://ex/Astronomer>×2, <http://ex/Artist>×1, <http://ex/Organisation>×1
```

Use `--level k` to isolate a specific zoom level:

```text
$ rete summary people.rete --level 1
schema pyramid level 1 — depth 1 (round 0), 2 class(es):
           3  <http://ex/Person>
           1  <http://ex/Organisation>
```

### The Index-Free HTTP Advantage

The true magic happens over HTTP. Using `rete summary-url`, a client reads the header, dictionary, and pyramid-meta sections. **The triple index remains entirely untouched.**

```text
$ rete summary-url https://host/people.rete
...
fetched 27337 of 161420 bytes in 3 range request(s) — index NOT fetched
```
A massive gigabyte-scale graph provides its zoomable type map for the exact same network cost (a few kilobytes) as a tiny graph!

## Advanced Features

*   **Multiple Inheritance:** Real ontologies aren't strict trees (e.g., `Astronaut` is both a `Scientist` and an `Explorer`). The pyramid retains *all* parent links, maintaining a true directed acyclic graph (DAG).
*   **Lateral Links:** Relationships like `Person memberOf Organisation` are also rolled up. At Level 0, this might generalize to `Agent memberOf Agent`. The connections are preserved at every zoom level.
*   **Missing Hierarchy?** If your data lacks `subClassOf` axioms, the pyramid gracefully degrades to a single flat level (identical to the Dataset Card's class list). 

## Is the Pyramid Worth It?

*   **The Schema Pyramid (Semantic Zoom) is always worth it.** It tracks the ontology size, not the graph size, meaning it remains tiny (tens of KB) and lightning-fast (~20ms read) at any scale.
*   **The Community Pyramid (Topological Zoom) is expensive.** It scales with the size of the graph, increasing build times and file size. 

**Rule of Thumb:** Keep the pyramid enabled for index-free exploration and overviews. If you are *only* serving selective backend SPARQL queries at massive scale, use the `--no-pyramid` flag to save space and build time.
