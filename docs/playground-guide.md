# The Rete Playground Guide

**[▶ Launch the Playground](playground.html)**

The Rete Playground is a single, static HTML page that runs the full Rete engine in WebAssembly. It requires no server, no accounts, and no installation. It even works completely offline! 

It is the fastest way to experience Rete: you can instantly query over 40 published datasets (including ontologies, citation networks, and 3D museum collections) directly in your browser. Large datasets are **range-read lazily**—meaning queries fetch only the specific bytes they need, keeping network traffic minimal.

## Loading a Dataset

The dataset picker categorizes available data by theme. When you select a dataset:
1.  **Examples:** Its pre-configured example queries appear as one-click chips.
2.  **Load Modes (for large datasets):**
    *   **Embedded:** Uses data bundled directly inside the web page.
    *   **Lazy:** (Default) Range-reads only what the query touches over HTTP.
    *   **Cache:** Downloads the entire `.rete` file to your browser's IndexedDB, enabling instantaneous, offline querying for all future sessions.

**Bring your own data:** Click **+ Add source → Connect (lazy)** and paste the URL of any `.rete` file hosted on a CORS-enabled server. Alternatively, use the **Build** tab to convert your own raw RDF (N-Triples, Turtle) into a queryable `.rete` file directly in the browser!

## Understanding the Data: 🏷 Card

Before writing a single query, click the **🏷 Card** button. This opens the embedded [Dataset Card](dataset-cards.md). 

Instantly, without downloading the triple index, you will see:
*   Title, description, license, and provenance.
*   Statistics: Triple counts, exact vocabularies, and class/predicate distributions.
*   The **Build Record**: Details on exactly when and how the file was generated, including the exact byte and millisecond cost of executing the starter queries.

## Writing SPARQL Queries

The Playground features a powerful SPARQL editor with syntax highlighting and auto-complete.

### Handy Editor Tools
*   **🔎 Find a term:** Instantly search the dataset's classes, predicates, and entity labels. Click any result to drop its IRI directly into your query.
*   **Labels Chip:** Automatically translates raw IRIs into human-readable labels inline (e.g., `wd:Q937` displays as *Albert Einstein*).
*   **🧠 Reason:** Toggles OWL 2 QL reasoning. This transparently rewrites your query to include entailed solutions (e.g., subclass relationships) without requiring physical data materialization.
*   **⛁ All Graphs:** Mounts the file so patterns outside of a `GRAPH` block match the union of the default graph *and* every named graph. (Note: This is non-standard SPARQL, but highly useful for N-Quads dumps).
*   **✨ SPARQL AI:** Drafts queries from plain English using a small language model that runs **locally on your GPU** (WebGPU). No data is sent to external APIs!

## Rich Output Views

Change the **Output** dropdown to visualize your SPARQL results in different formats. *Note: Ensure your query `SELECT`s the appropriate variables to power these views.*

| Output View | Required Query Shape |
| :--- | :--- |
| **Table** | Any `SELECT`, `ASK`, or `CONSTRUCT`. |
| **Cards** | Any query. Displays one styled card per row; excellent for mobile viewing and multimedia. |
| **Graph** | Needs edges. Use `CONSTRUCT` or a `SELECT` with 2 (nodes) or 3 (subject, predicate, object) variables. |
| **Map** | Needs a WKT geometry column bound to a variable. |
| **Time** | Needs a year/date column (`xsd:gYear`, `xsd:date`, etc.). Creates a multi-year heatmap. |
| **TTL / JSON-LD** | Needs a `CONSTRUCT` query to serialize the resulting triples. |

## Specialized Exploration Modes

The mode strip allows you to analyze the dataset beyond standard SPARQL:

*   **Schema:** Views the dataset's effective schema and class relationships via the index-free semantic zoom pyramid.
*   **Explore:** Browse per-class entity tables. If the dataset has a Parquet companion, you can run DuckDB/SQLite SQL queries alongside SPARQL.
*   **Semantic:** Perform natural-language search over the dataset's meaning using local sentence embeddings.
*   **SHACL:** Validate the graph against specific shapes.
*   **Reach:** Analyze transitive reachability from seed entities.
*   **Coherence:** Run tiered OWL coherence checks.

## Sharing and Federation

**Federation:** Click **+ Add source** to layer multiple `.rete` files or live SPARQL endpoints together. Your query will execute as a unified, cross-source join. 

**Sharing:** The Playground keeps its entire state (dataset, query, toggles, view mode) in the URL fragment. 
*   Click the 🔗 icon on an example query to copy a clean, pre-rendered preview link (e.g., `q/dataset-name.html`). 
*   If you write a custom query, clicking **Share** copies the full deep-link URL so anyone can reproduce your exact environment.

## Browser Recommendations

For the best experience, especially on multi-gigabyte remote datasets, **we highly recommend Chromium-family browsers** (Chrome, Edge, Brave). 

Chromium supports **concurrent range reads** via Asyncify, allowing the engine to fetch multiple byte ranges in parallel. Firefox and Safari default to sequential reads to avoid WebAssembly stack limitations, making large remote queries noticeably slower.
