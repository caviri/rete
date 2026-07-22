# obo — OBO Foundry ontology harvest

Downloads every OBO Foundry ontology (https://obofoundry.org/) into
`data/obo/raw/`. OBO Foundry is a registry of ~265 open-licensed biomedical
ontologies (GO, ChEBI, HPO, Uberon, DOID, Mondo, CL, PR, NCBITaxon, …). No
scraping: the registry is a machine-readable JSON-LD file and every ontology has
a canonical download via `purl.obolibrary.org`.

## Source

| What | URL |
|---|---|
| Registry (all metadata) | `http://purl.obolibrary.org/meta/ontologies.jsonld` (+ `.yml` mirror on GitHub) |
| Per-ontology main product | each entry's `ontology_purl`, e.g. `http://purl.obolibrary.org/obo/go.owl` |

`ontology_purl` is the "main OWL edition" — self-contained RDF/XML, the same
artifact `rete build` ingests directly (it reads `.owl`/`.rdf`/`.rdfxml`). Each
ontology also publishes `.obo` and `.json` editions in its `products` list; this
harvester grabs the primary `.owl` only.

## Run

```
python scripts/obo/harvest.py            # 189 active ontologies (~10.5 GB), resumable
python scripts/obo/harvest.py --all      # + 76 obsolete/orphaned (many 404)
python scripts/obo/harvest.py --survey   # print the plan, download nothing
python scripts/obo/harvest.py --limit 5  # smoke test
```

Output layout in `data/obo/raw/`:

```
_registry/ontologies.jsonld   full registry
_registry/ontologies.yml      YAML mirror
_registry/manifest.json       what we fetched: id -> {url, resolved, bytes, license, title}
<id>/<id>.owl                 e.g. go/go.owl, chebi/chebi.owl
_errors.jsonl                 failed downloads after retries
```

Resumable: existing non-empty files are skipped; re-run to retry only what's
missing. 8 threads. Biggest ontologies: ncbitaxon (~2 GB), gaz (~1.3 GB),
pr (~1.3 GB), chebi (~0.9 GB), ncit (~0.8 GB), dron (~0.7 GB).

## Next

Any `.owl` here builds straight to a queryable `.rete`:
`rete build data/obo/raw/hp/hp.owl -o web/hp.rete --pyramid-algo types --card`.
For OWL/XML-only ontologies (not RDF/XML) use `scripts/owl_to_nt.py` (owlready2)
first. See the `rete-from-graph` skill.
