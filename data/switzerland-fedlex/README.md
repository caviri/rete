# switzerland-fedlex

The **Swiss federal law** knowledge graph — Fedlex, the official publication platform
of the Swiss Confederation — as Linked Data (JOLux + ELI ontologies).

- Source (SPARQL): **https://fedlex.data.admin.ch/sparqlendpoint** (OpenLink Virtuoso)
- Portal: https://www.fedlex.admin.ch/ · Linked-data docs: https://fedlex.data.admin.ch/
- License: **Fedlex terms** — federal legislation is free to use/reproduce; cite
  *"Source: Fedlex, the publication platform of Swiss federal law"*. Ontology bundle
  (JOLux/ELI/SKOS) is openly published.
- Snapshot: harvested 2026-07-30. **56,321,136 triples across 497,896 named graphs.**

## Two layers

Fedlex exposes the law as **two complementary layers**:

1. **Metadata RDF KG** (this snapshot) — the queryable graph: acts, their
   consolidated versions, dates, ELI identifiers, titles, languages, subject
   classifications (JURIVOC / legal-taxonomy), citations and amendment relations.
   Modelled with **JOLux** (`http://data.legilux.public.lu/resource/ontology/jolux#`,
   the Luxembourg-origin legislation ontology Fedlex reuses) + **ELI** (European
   Legislation Identifier) + SKOS for the vocabularies.
2. **Full text in Akoma Ntoso XML** (OASIS *LegalDocML*) — the actual article text of
   each act, one XML per act/version/language (e.g. `eli/cc/2013/643/en`). NOT
   harvested here; it is a separate, much larger text layer. This snapshot is the
   metadata KG that indexes it.

## Layout

```
data/switzerland-fedlex/
  README.md
  SHA256SUMS.txt
  raw/
    quads/                       # the KG: gzipped N-Quads shards (part-NNNN.nq.gz)
      _progress.json             # resumable-harvest cursor
    graphs.txt                   # the 497,896 named-graph URIs (enumeration output)
    ontology/                    # the TBox
      jolux_ontology.zip
      jolux-ontology-owl/
        jolux.ttl                # JOLux ontology (741 KB)
        eli-v1.1.owl             # ELI ontology
        skos.owl  skos-xl.ttl  event.owl  prov.ttl  catalog-v001.xml
  scripts/
    download.sh                  # ontology + SPARQL harvest (Docker)
    fetch_sparql.py              # the Virtuoso-aware harvester
```

## How it was harvested (Fedlex offers NO static RDF dump)

The only access to the RDF is the SPARQL endpoint, which is **Virtuoso** with two hard
limits the harvester (`scripts/fetch_sparql.py`) designs around:

- **`ORDER BY` is capped at 10,000 rows** (error *SR353*) → no global keyset sort is
  possible; instead graphs are enumerated with `GROUP BY ?g LIMIT 10000 OFFSET N`
  (index-backed, fast, stable) and deduped.
- **`ResultSetMaxRows = 100,000`** → every response is silently truncated at 100k.
  So quads are pulled per **batch of graphs** with a **COUNT guard** (`rows == COUNT`,
  else recurse-split); a single graph larger than the cap is paged with per-graph
  `OFFSET`.

Data is requested as **SPARQL-JSON** (not CSV) to preserve each term's type
(uri/bnode/literal), `xml:lang` and datatype, then serialized to faithful N-Quads.

## Reproduce

```bash
# full run (ontology + ~6–10h SPARQL harvest into raw/quads/*.nq.gz), resumable
bash data/switzerland-fedlex/scripts/download.sh

# just re-enumerate the named-graph list
docker run --rm -v "$PWD:/work" -w //work python:3.12-slim \
  python data/switzerland-fedlex/scripts/fetch_sparql.py --enumerate-only
```

## Next step

Build `switzerland-fedlex.rete` (hand off to rete-from-graph): stream
`zcat raw/quads/*.nq.gz` into a memory-bounded `rete build -`. The named graphs map
cleanly onto quads; the JOLux/ELI TBox in `raw/ontology/` gives the class/property
vocabulary for the pyramid and for optional OWL-QL reasoning.
