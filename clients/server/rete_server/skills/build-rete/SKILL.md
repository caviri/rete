---
name: build-rete
description: Build a complete, trustworthy .rete knowledge graph from data, a spreadsheet, notes or a conversation. Use when the user wants to turn something into a queryable graph file, model data as RDF, write an ontology, create a dataset card with license, creators and DOI, add example queries and SHACL shapes, or publish a graph. Covers the server tools (suggest_vocabulary, check_ontology, build_rete) and the local rete CLI, and how to verify the result before handing it over.
---

# Build a complete .rete

A finished graph is more than triples. It has a model other people can read,
entities with labels and links, a Dataset Card (title, license, source, creators,
DOI), starter queries that run, shapes that check it, and a verification you
actually did. The steps:

1. Model the data (classes, properties, IRIs, reused vocabularies).
2. Write the instances.
3. Write the card and the starter queries.
4. Write SHACL shapes.
5. Build.
6. Verify, fix, rebuild.

[references/worked-example.md](references/worked-example.md) is a complete small
case you can copy: instances, `build_rete` arguments, shape, expected results.
It is run by the tests, so it works.

## Which path

**Server tools** build small graphs (input up to 4 MB of RDF text by default) inside
the conversation. The file is ephemeral: it lives under `generated/<hash>.rete`
until the server restarts or evicts it, and the dataset key works at once in
every query tool. Ask for the file with `include_base64=true` to give the user a
copy. The tools are `suggest_vocabulary`, `check_ontology`, `build_rete` and
`causal_diagram`. The server can run with authoring off; then those four tools
are not in the tool list. If `build_rete` is missing, say so and use the local
path.

**Local CLI** builds anything, from a hundred triples to billions. It writes the
file where you choose, with the fuller card (derived profile, about twenty
starter queries, cost measurements) and a label index. Use it for a graph that
will be published. Docker image: `ghcr.io/caviri/rete-cli`; or the `rete` binary
(see the repository's install guide).

Both read Turtle, N-Triples, N-Quads, TriG, RDF/XML and JSON-LD.

## 1. Model

Decide, on paper, before any triples:

- What are the **things** (people, papers, places)? Each becomes a class.
- What **links** things to things (author, cites, locatedIn)? Each becomes an object property.
- What **facts** does a thing carry (name, date, count)? Each becomes a datatype property with a typed literal.
- What question should the graph answer? Write three. If the model cannot answer them, change it now.

Reuse vocabularies. On the server, `suggest_vocabulary(query)` searches Linked
Open Vocabularies for existing terms (it calls out to the internet). The
cheat sheet, IRI rules, typed literals and a CSV-to-Turtle script are in
[references/modeling.md](references/modeling.md). Rules that matter most:

- Every entity has `rdf:type` and `rdfs:label`. The label is what people see and what `find_entities` searches.
- Links go to IRIs, not strings. A name in a string cannot be joined.
- IRIs are minted under a base you control, readable and stable.
- Dates and numbers are typed literals.
- Link out (`schema:sameAs` to Wikidata, ORCID, ROR) where you can.

If you define classes and properties of your own, write them down in Turtle
(label, comment, domain, range, `rdfs:subClassOf` an existing class) and run
`check_ontology(ontology)` until `ok` is true. Fix errors; read warnings and
decide.

## 2. Instances

Write Turtle (compact) or N-Triples (easy to generate). Build from a table with
a script, not by hand, and keep the script: it is the provenance. Small
graphs in the conversation: write them out. Larger: generate and build locally.

```turtle
@prefix rdfs:   <http://www.w3.org/2000/01/rdf-schema#> .
@prefix schema: <https://schema.org/> .
@prefix lib:    <https://example.org/library/> .

lib:ada-lovelace a schema:Person ;
    rdfs:label "Ada Lovelace" ;
    schema:sameAs <https://www.wikidata.org/entity/Q7259> .

lib:notes-on-the-engine a schema:ScholarlyArticle ;
    rdfs:label "Notes on the Analytical Engine" ;
    schema:author lib:ada-lovelace .
```

Test the parse as you go. The server tool reports the position of a parse
error; locally `rete validate data.ttl` does.

## 3. Card and starter queries

Give the card everything you know and nothing you do not: title, description,
license, source, version, creators with ORCID, publisher with ROR, DOI when
there is one, keywords, and a citation. The field list, what a good description
says and how to write starter queries are in
[references/card.md](references/card.md).

Write three to six starter queries, simplest first, each with a `title`, a
plain `question` and full `PREFIX` lines. Run each one before you ship it.

Never invent a DOI, an ORCID or a license. If the user has not given you one,
leave the field out and say it is missing.

## 4. Shapes

SHACL shapes state what must be true of the data. They catch the mistakes a
parser cannot: a paper with no author, two years, a link to a string.

```turtle
@prefix sh:     <http://www.w3.org/ns/shacl#> .
@prefix xsd:    <http://www.w3.org/2001/XMLSchema#> .
@prefix rdfs:   <http://www.w3.org/2000/01/rdf-schema#> .
@prefix schema: <https://schema.org/> .

schema:PersonShape a sh:NodeShape ;
    sh:targetClass schema:Person ;
    sh:property [ sh:path rdfs:label ; sh:minCount 1 ; sh:maxCount 1 ; sh:datatype xsd:string ] ;
    sh:property [ sh:path schema:birthDate ; sh:maxCount 1 ; sh:datatype xsd:date ] ;
    sh:property [ sh:path schema:sameAs ; sh:nodeKind sh:IRI ;
                  sh:pattern "^https://www\\.wikidata\\.org/entity/Q[0-9]+$" ] .

schema:ArticleShape a sh:NodeShape ;
    sh:targetClass schema:ScholarlyArticle ;
    sh:property [ sh:path schema:author ; sh:minCount 1 ; sh:class schema:Person ] ;
    sh:property [ sh:path schema:datePublished ; sh:minCount 1 ; sh:maxCount 1 ;
                  sh:datatype xsd:gYear ] .
```

Common constraints: `sh:minCount` and `sh:maxCount` (required, at most one),
`sh:datatype`, `sh:class` (the linked entity has this type), `sh:nodeKind`
(`sh:IRI`, `sh:Literal`), `sh:pattern`, `sh:in`, `sh:minInclusive` and
`sh:maxInclusive`. Run them with `validate_shacl(dataset, shapes)`; only the
targets' bytes are fetched. Locally: `rete shacl data.rete --shapes shapes.ttl`.
Keep shapes with the data (in the repository, next to the build script) so the
check reruns on every rebuild.

## 5. Build

### On the server

```json
{
  "rdf": "<the Turtle text>",
  "format": "ttl",
  "card": { "title": "Early computing papers", "description": "...", "license": "CC0-1.0",
            "source": "https://example.org/library/", "version": "2026-11-28",
            "creators": [{ "name": "Ada Lovelace", "orcid": "https://orcid.org/0000-0002-1825-0097" }],
            "keywords": ["computing history"] },
  "examples": [{ "title": "Papers by year", "question": "Which papers, oldest first?",
                 "sparql": "PREFIX ..." }],
  "text_index": false,
  "include_base64": false
}
```

The answer has the dataset key (`generated/<hash>.rete`), the counts (`quads`,
`terms`) and a `content_hash`. Set `text_index=true` when the graph has long
text you want to search by word. Set `include_base64=true` to hand over the file.

### Locally

```sh
rete validate data.ttl
rete build data.ttl -o data.rete --pyramid-algo types --label-index --card-file card.json
```

`card.json` holds the fields from [references/card.md](references/card.md), with
`"example_queries": ["SELECT ..."]`. The flags: `--pyramid-algo types` groups by
class (use it for typed graphs), `--label-index` makes names searchable (it
reads `rdfs:label`, `skos:prefLabel`, `foaf:name`, `dcterms:title`,
`schema:name`), `--text-index` adds word search, and `--card` (implied by
`--card-file`) writes the card. For an input too big for memory add
`--memory-budget-mb 4096 --tmp-dir /tmp/spill`; `rete estimate` predicts the
size and time first. With Docker:

```sh
docker run --rm -v "$PWD:/w" -w /w ghcr.io/caviri/rete-cli:0.7.8 \
  build data.ttl -o data.rete --pyramid-algo types --label-index --card-file card.json
```

To query a local file through the server, put it in the folder the server
serves (`RETE_DATA_DIR`); it appears as `local/<name>.rete`.

## 6. Verify

Do these, in this order, and report what you saw.

1. **Counts.** `dataset_card` shows `triple_count`. It should equal what you sent (the worked example: 34 triples in, 34 out). Fewer means duplicates or a parse that dropped something; more means entailment you did not intend.
2. **Schema.** `dataset_schema(dataset)` lists the classes and relations. Every class you modelled should appear with the instance count you expect; a class missing or with a different IRI is a typo in a prefix. Relations should connect the classes you meant (`ScholarlyArticle` to `Person` through `author`), not `(literal)` where you meant a link.
3. **Card.** Read `dataset_card` back: title, license, version, creators, publisher, DOI, description, keywords, source. Compare with what the user gave you.
4. **Starter queries.** `example_queries(dataset)`, then run each with `sparql_query`. Every one returns rows. Check `warnings` is empty.
5. **Shapes.** `validate_shacl(dataset, shapes)` gives `conforms: true`, or a list of results (focus node, path, constraint) to fix at the source.
6. **Names.** `find_entities(dataset, "<a name from your data>")` returns the IRI. If not, the entity has no label.
7. **A real question.** Answer one of your three questions from step 1 with `sparql_query`. If you cannot, the model is wrong.
8. **Locally**, also `rete verify data.rete` (content hash and every section) and `rete card-audit data.rete` (which starter queries still answer).

Fix at the source (the script or the Turtle) and rebuild. Do not patch the
graph.

## 7. Hand over

Give the user: the dataset key or the file, the counts, what you verified and
what you could not (a DOI that does not exist yet, a license to confirm), and
how to query it (`query` skill). If the file is going on the web, it needs a host
that serves HTTP `Range` and CORS; see the `web-pages` skill.
