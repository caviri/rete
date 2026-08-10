# Graph Data, For Dummies

Welcome! This is a gentle, practical tour of graph data formats. We will focus on one running question: **what kinds of questions does this let me ask?**

If you have never touched RDF, SPARQL, or Neo4j before, this is the perfect place to start. By the end, you'll know exactly what a `.rete` file is and what you can do with it.

## What is a Graph?

<figure class="fig-right">
  <img src="img/graph-vs-table.svg" alt="The same facts shown as a relational table of rows on the left and as a node-link graph on the right, joined by a 'same facts' arrow.">
  <figcaption>The same facts as rows vs. as a graph. A graph stores the relationships directly, so "who is connected to whom" is a traversal, not a join.</figcaption>
</figure>

A graph (also known as a network) is made of just two ingredients:
- **Nodes**: The *things* (e.g., a person, a software package, a disease).
- **Edges**: The *relationships* between things (e.g., Alice *knows* Bob; `app` *dependsOn* `logging`).

That's it. Whether it's a social network, an org chart, or a knowledge base, they all share this shape: dots connected by lines. 

### Why Use a Graph?
Graphs are built to answer questions that are extremely painful to answer using standard spreadsheets or SQL tables:
- **Direct Connections:** "Who is directly connected to X?" 
- **Reachability & Paths:** "Is there *any* path from A to B, no matter how long?" (e.g., "Does our app eventually rely on a vulnerable library?")
- **Structure & Clusters:** "What communities exist in this data?"

**Rule of thumb:** Tables are great for finding "all rows where age > 30". Graphs are great for exploring "everything reachable from here."

## The Two Families: RDF vs. Labeled Property Graphs

There are two dominant ways to write down graph data.

<figure class="fig-right">
  <img src="img/triple.svg" alt="An RDF triple drawn as a node-link: a subject node connected by a labeled predicate arrow to an object node, with a second example pointing to a literal value.">
  <figcaption>One triple = one fact: <code>subject —predicate→ object</code>. Resources are rounded nodes; literals are boxes. A graph is many triples sharing nodes.</figcaption>
</figure>

### 1. RDF (Resource Description Framework)
RDF breaks the world down into **triples**—short statements consisting of a `(subject, predicate, object)`:
```text
<Alice>  <knows>  <Bob>
<Alice>  <age>    "30"
```
In RDF, everything (even the relationship name itself) is a first-class resource with a global identifier (a URL). It is the W3C standard for sharing data across the web.

### 2. Labeled Property Graphs (LPG)
LPGs (used by databases like Neo4j) give nodes and edges **labels** and key-value **properties**:
```text
(:Person {name:"Alice", age:30}) -[:KNOWS {since:2019}]-> (:Person {name:"Bob"})
```
The superpower of LPGs is that edges can hold properties (like `since:2019`). Plain RDF requires extra modeling to do this.

| Feature | RDF (Triples) | Labeled Property Graph (LPG) |
|---|---|---|
| **Atom of data** | A simple statement `(s, p, o)` | A node or edge with properties |
| **Identity** | Global URLs (web-wide sharing) | Local IDs (app-specific) |
| **Edge data?** | Not directly | Yes |
| **Query Language** | **SPARQL** (W3C standard) | Cypher / GQL |
| **Best at** | Merging data, inference, sharing | Rich edges, app-local traversals |

> **Note:** `rete` is built for **RDF**. It uses standard RDF data models and standard SPARQL queries. (However, it can translate simple Cypher read queries into SPARQL for compatibility).

## The Graph Standards Ecosystem

The RDF world involves several W3C standards. Here is how `rete` uses them:

- **RDF (Data Model):** Answers "What facts do I have?" ✅ (`rete`'s core format).
- **RDFS (Lightweight Schema):** Answers "If Dog is an Animal, is Rex an Animal?" ✅ (Supported via `rete reason`).
- **SPARQL (Query Language):** Answers "Find everyone Alice transitively knows." ✅ (Supported).
- **SHACL (Validation):** Answers "Does every Person have exactly one email?" ✅ (Supported via `rete shacl`).
- **N-Triples / Turtle / JSON-LD (Formats):** Answers "How do I save this to disk?" ✅ (Supported for building and exporting).

## What Questions Can You Ask `rete`?

Here is a practical guide to the kinds of questions you can ask your data using `rete`'s CLI. *(Examples assume a built graph named `deps.rete`)*.

### 1. Point Lookups (Triple Patterns)
"What facts share this exact relationship?"
```sh
# Omitting the subject and object turns them into wildcards
rete query deps.rete --predicate '<http://ex/hasVulnerability>'
```

### 2. Multi-Hop Joins (Basic Graph Patterns)
"Find me a 2-hop chain."
```sh
# ?x, ?y, and ?z are variables. We want to find A -> B -> C chains.
rete bgp deps.rete "?x <http://ex/dependsOn> ?y . ?y <http://ex/dependsOn> ?z"
```

### 3. Reachability (Property Paths)
"What transitively depends on X?" (This is what tables struggle with!)
```sh
# The '+' symbol tells SPARQL to follow the edge one or more times
rete sparql deps.rete "PREFIX e: <http://ex/> SELECT ?d WHERE { ?d e:dependsOn+ e:log4x }"
```

### 4. Aggregations
"How many dependencies does each package have, ordered by most to least?"
```sh
rete sparql deps.rete \
  "PREFIX e: <http://ex/> SELECT ?p (COUNT(?d) AS ?deps) WHERE { ?p e:dependsOn ?d } GROUP BY ?p ORDER BY DESC(?deps)"
```

### 5. Big Picture overviews
"What kinds of things exist here, and how do they relate?"

<img src="img/pyramid.svg" alt="The rete pyramid: a coarse community summary at the top, communities in the middle, and full triples at the base; a client reads the top first and drills down only where needed.">

`rete` builds a **pyramid index** of your data so you can get high-level summaries instantly, without reading millions of rows.
```sh
rete schema deps.rete     # Shows the dataset's logical schema
rete summary deps.rete    # Shows the structural overview
```

### 6. Logical Coherence
"Does my data logically contradict itself?"
```sh
# Flags incoherent points (e.g., an item belonging to two mutually exclusive classes)
rete reason deps.rete
```

## Where `rete` Fits In

`rete` is a **format and query layer** that completely eliminates the database server. 
You publish a single, immutable `.rete` file to a URL (like S3 or GitHub), and clients query it directly over the network. They fetch only the byte ranges they need to answer the question.

**Ready to start?**
- Head to **[Getting Started](getting-started.md)** to build your first graph.
