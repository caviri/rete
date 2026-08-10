# Dataset Cards

A **Dataset Card** is a compact, self-describing metadata record embedded directly *inside* a `.rete` file. It transforms an opaque file into a rich data catalog entry. 

With a single `rete card` (or `rete info`) command, you can instantly see:
*   Who created the dataset and under what license.
*   The original source and publication date.
*   Automatically derived statistics: total counts, predicates, and classes.
*   The core schema structure.

Because the card is embedded inside the file (in the metadata section right after the header), it is **tamper-evident** and folds perfectly into the file's `blake3` content hash.

## Building a Card

Adding a card is strictly opt-in and does not change the core `.rete` format.

**Command Line Flags:**
You can supply basic metadata directly via flags. Doing so tells the builder to calculate the derived statistics automatically.
```sh
# Auto-derived stats only:
rete build data.nt -o data.rete --card

# With curated catalog metadata:
rete build data.nt -o data.rete \
  --title "Citation graph 2021" \
  --license "CC0-1.0" \
  --source "https://example.org/citations" \
  --description "Open citations sharded by year" \
  --created 2026-06-08
```

**JSON Card File:**
For comprehensive metadata (like creators, publishers, and example queries), use a JSON file via the `--card-file` flag:
```json
{
  "title": "Citation graph 2021",
  "license": "CC0-1.0",
  "source": "https://example.org/citations",
  "description": "Open citations sharded by year",
  "version": "2021.2",
  "creators": [
    { "name": "Ada Lovelace", "orcid": "https://orcid.org/0000-0002-1825-0097" }
  ],
  "publisher": { "name": "EPFL", "ror": "https://ror.org/02s376052" },
  "keywords": ["citations", "open science", "scholarly communication"],
  "theme": ["http://publications.europa.eu/resource/authority/data-theme/TECH"],
  "example_queries": [
    "SELECT ?citing WHERE { ?citing <http://purl.org/spar/cito/cites> ?cited }"
  ],
  "extra": {
    "internal_id": "DS-2021-042"
  }
}
```

```sh
rete build data.nt -o data.rete --card-file card.json --title "Override title"
```
*Note: Explicit CLI flags will override the corresponding fields in your JSON file.*

## What's Inside a Card?

A Dataset Card merges **curated fields** (provided by you) with **derived fields** (computed by Rete during the build).

### Curated Fields (Provided by you)
*   **Core Metadata:** `title`, `license`, `source`, `created`, `description` (Markdown, capped at 8 KiB).
*   **Identity & Provenance:** `version` (your dataset version), `doi`, `cite_as`, `creators`, `publisher`, `source_date`, `derived_from`.
*   **Classification:** `keywords` (free text) and `theme` (controlled vocabulary IRIs).
*   **Usage:** `example_queries` (sample SPARQL queries).
*   **Custom Fields:** The `extra` object (for private/internal metadata).

### Derived Fields (Computed by Rete)
*   **Counts:** `triple_count`, `quad_count`, `named_graph_count`, `term_count`.
*   **Structure:** `predicates`, `classes`, `vocabularies`, `datatypes`, `languages`, `class_links` (the effective schema).
*   **Top Hubs:** `top_hubs` (by out-degree), `in_hubs` (by in-degree).
*   **Signals:** `label_predicate`, `geo_wkt`, `temporal_extent`, etc.
*   **Queries:** A tiered library of automatically generated starter queries.
*   **Format:** `format_version`.

> **Note on Derived Fields:** The per-predicate and per-class stats are computed over the **default graph** only.

## Themes: Using Controlled Vocabularies

The `theme` field requires a valid **IRI** from a controlled vocabulary, not free text. If you want to use free text, put it in `keywords` instead. 

**Recommended Vocabularies:**
*   **EU Data Themes:** `http://publications.europa.eu/resource/authority/data-theme/GOVE` (The default for government/open-data).
*   **Wikidata:** `https://www.wikidata.org/entity/Q413` (The ultimate fallback for almost any topic).
*   **Domain Specific:** Use MeSH for clinical data, AGROVOC for agriculture, or GeoNames for places.

## The Description: Markdown, not HTML

The `description` field supports standard Markdown syntax (headings, lists, blockquotes, code, and links). 

*   **Security First:** Raw HTML is intentionally escaped and ignored. This prevents anyone from injecting malicious `<script>` tags into a shared dataset card. 
*   **Size Limit:** Descriptions are capped at 8 KiB (roughly 1,300 words). If you need more, provide a summary here and link to the full documentation.

## Custom Fields (`extra`)

Need to track internal review statuses or pipeline tags? Use the `extra` object!

**The Golden Rules of Custom Fields:**
1.  **Is there a standard term for it?** If yes, ask for it to become a primary field. The `extra` bag strips semantic meaning, so standard metadata parsers will ignore it.
2.  **Can it be derived?** Never curate data that can be calculated automatically (e.g., language tags). 

**Limits:** The entire `extra` bag is capped at 8,192 bytes, 64 keys, and a maximum nesting depth of 2. Exceeding these limits will fail the build.

## Build Information

Every carded build generates an adjacent, unhashed **Build Info** section containing:
*   `built_at`: The exact UTC timestamp of the build.
*   `builder`: The CLI version used (e.g., `rete-cli 0.3.2`).
*   `params`: The specific CLI flags used.
*   `query_costs`: Measured execution costs (bytes, requests, ms) for every starter query.

This section is kept *outside* the `blake3` hash so that rebuilding the exact same data produces a byte-identical file, despite the differing timestamp.

## Generating Starter Queries

Rete automatically generates a library of starter SPARQL queries customized for your specific dataset's vocabulary (e.g., finding the most populous class and using it to draft a template). 

*   **Verified to Work:** During the build, Rete actively runs these queries cold. If a query returns zero rows, it is automatically dropped. This guarantees that every shipped starter query actually works.
*   **Named-Graph Aware:** If your dataset only contains named graphs (no default graph), Rete automatically rewrites the starter queries to use `GRAPH ?g { ... }`.

### Re-Carding an Existing File

You can fix or update a card without reprocessing the original source RDF! Use the `repyramid` command to rewrite the file losslessly:

```sh
rete repyramid catalog.rete -o catalog-fixed.rete \
  --card --title "National Open Data Catalog" --license "CC0-1.0"
```
