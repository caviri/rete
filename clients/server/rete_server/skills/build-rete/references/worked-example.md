# A worked example: early computing papers

Small enough to read in one screen, complete enough to copy: instances typed and
labelled with schema.org terms, links as IRIs, typed dates, a full card, four
starter queries and a SHACL shape. The tests build exactly this and run every
query.

## 1. Instances (Turtle)

Reuse schema.org (`schema:Person`, `schema:ScholarlyArticle`, `schema:author`) and
name things with `rdfs:label`, which every tool shows and searches.
Mint IRIs only for your own entities, under a base you control. Link to
Wikidata with `schema:sameAs` where you know the entity.

```turtle
@prefix rdfs:   <http://www.w3.org/2000/01/rdf-schema#> .
@prefix xsd:    <http://www.w3.org/2001/XMLSchema#> .
@prefix schema: <https://schema.org/> .
@prefix lib:    <https://example.org/library/> .

lib:ada-lovelace a schema:Person ;
    rdfs:label "Ada Lovelace" ;
    schema:birthDate "1815-12-10"^^xsd:date ;
    schema:sameAs <https://www.wikidata.org/entity/Q7259> .

lib:charles-babbage a schema:Person ;
    rdfs:label "Charles Babbage" ;
    schema:birthDate "1791-12-26"^^xsd:date ;
    schema:sameAs <https://www.wikidata.org/entity/Q46633> .

lib:alan-turing a schema:Person ;
    rdfs:label "Alan Turing" ;
    schema:birthDate "1912-06-23"^^xsd:date ;
    schema:sameAs <https://www.wikidata.org/entity/Q7251> .

lib:grace-hopper a schema:Person ;
    rdfs:label "Grace Hopper" ;
    schema:birthDate "1906-12-09"^^xsd:date ;
    schema:sameAs <https://www.wikidata.org/entity/Q11641> .

lib:notes-on-the-engine a schema:ScholarlyArticle ;
    rdfs:label "Notes on the Analytical Engine" ;
    schema:author lib:ada-lovelace , lib:charles-babbage ;
    schema:datePublished "1843"^^xsd:gYear ;
    schema:pagination "40" .

lib:computable-numbers a schema:ScholarlyArticle ;
    rdfs:label "On Computable Numbers" ;
    schema:author lib:alan-turing ;
    schema:datePublished "1936"^^xsd:gYear ;
    schema:citation lib:notes-on-the-engine .

lib:education-of-a-computer a schema:ScholarlyArticle ;
    rdfs:label "The Education of a Computer" ;
    schema:author lib:grace-hopper , lib:alan-turing ;
    schema:datePublished "1952"^^xsd:gYear ;
    schema:citation lib:computable-numbers , lib:notes-on-the-engine .
```

## 2. The `build_rete` call

The arguments, as JSON. `rdf` is the Turtle from step 1 (left out here); `card` and
`examples` go in as written.

```json
{
  "format": "ttl",
  "text_index": false,
  "card": {
    "title": "Early computing papers",
    "description": "Four people, three papers and who cited whom, from the first notes on the Analytical Engine to the education of a computer.\n\nA teaching example: small on purpose.",
    "license": "CC0-1.0",
    "source": "https://example.org/library/",
    "version": "2026-11-28",
    "source_date": "2026-11-28",
    "creators": [
      { "name": "Ada Lovelace", "orcid": "https://orcid.org/0000-0002-1825-0097" }
    ],
    "publisher": { "name": "EPFL", "ror": "https://ror.org/02s376052" },
    "doi": "https://doi.org/10.5281/zenodo.0000000",
    "cite_as": "Lovelace, A. (2026). Early computing papers. https://doi.org/10.5281/zenodo.0000000",
    "keywords": ["computing history", "citations"]
  },
  "examples": [
    {
      "title": "Papers by year",
      "question": "Which papers were published, oldest first?",
      "sparql": "PREFIX schema: <https://schema.org/>\nPREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>\nSELECT ?paper ?title ?year WHERE {\n  ?paper a schema:ScholarlyArticle ;\n         rdfs:label ?title ;\n         schema:datePublished ?year .\n}\nORDER BY ?year"
    },
    {
      "title": "Who wrote what",
      "question": "Which people wrote which papers?",
      "sparql": "PREFIX schema: <https://schema.org/>\nPREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>\nSELECT ?person ?title WHERE {\n  ?paper schema:author ?p ;\n         rdfs:label ?title .\n  ?p rdfs:label ?person .\n}\nORDER BY ?person"
    },
    {
      "title": "Most cited papers",
      "question": "Which papers are cited most often?",
      "sparql": "PREFIX schema: <https://schema.org/>\nPREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>\nSELECT ?paper ?title (COUNT(?citing) AS ?citations) WHERE {\n  ?citing schema:citation ?paper .\n  ?paper rdfs:label ?title .\n}\nGROUP BY ?paper ?title\nORDER BY DESC(?citations)"
    },
    {
      "title": "Authors born before 1910",
      "question": "Who was born before 1910 and what did they write?",
      "sparql": "PREFIX schema: <https://schema.org/>\nPREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>\nPREFIX xsd: <http://www.w3.org/2001/XMLSchema#>\nSELECT ?name ?born WHERE {\n  ?p a schema:Person ;\n     rdfs:label ?name ;\n     schema:birthDate ?born .\n  FILTER(?born < \"1910-01-01\"^^xsd:date)\n}\nORDER BY ?born"
    }
  ]
}
```

## 3. A SHACL shape

Every paper has a title, one year and at least one author who is a person.

```turtle
@prefix sh:     <http://www.w3.org/ns/shacl#> .
@prefix rdfs:   <http://www.w3.org/2000/01/rdf-schema#> .
@prefix xsd:    <http://www.w3.org/2001/XMLSchema#> .
@prefix schema: <https://schema.org/> .

schema:ArticleShape a sh:NodeShape ;
    sh:targetClass schema:ScholarlyArticle ;
    sh:property [
        sh:path rdfs:label ;
        sh:minCount 1 ;
        sh:datatype xsd:string
    ] ;
    sh:property [
        sh:path schema:datePublished ;
        sh:minCount 1 ;
        sh:maxCount 1 ;
        sh:datatype xsd:gYear
    ] ;
    sh:property [
        sh:path schema:author ;
        sh:minCount 1 ;
        sh:class schema:Person
    ] .
```

## 4. What the checks should show

- `dataset_card`: the title, license, version, creators, publisher, DOI, keywords and the four examples you gave, and `triple_count` 34.
- `dataset_schema`: classes `schema:ScholarlyArticle` (3) and `schema:Person` (4); relations ScholarlyArticle to Person through `schema:author` (5) and ScholarlyArticle to ScholarlyArticle through `schema:citation` (3).
- Each example returns rows: 3, 5, 2 and 3.
- `validate_shacl` with the shape: `conforms` true.
- `find_entities(dataset, "Grace")`: the IRI `https://example.org/library/grace-hopper`.
