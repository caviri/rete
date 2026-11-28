# Modeling data as a graph

The aim is a graph other people and agents can query without asking you what a
column meant. The IRIs and classes you choose are the interface.

## From a table to triples

Each row is usually one entity; each column is a property or a link.

- A row becomes a subject with an IRI and an `rdf:type`.
- A column of plain values (a name, a count, a date) becomes a literal.
- A column that refers to another entity (an author, a country, a lab) becomes a link to that entity's IRI. Give the other entity its own row and label. A string where a link belongs cannot be joined.
- A column with several values in one cell ("Ada; Charles") becomes several triples.
- A relationship that has its own facts (a person's role in a project, with start and end dates) becomes its own node, with links to both sides and its own properties.

## IRIs

- Mint under a base you control: `https://example.org/library/`. Do not use `http://localhost/...` or a made-up domain you do not own, if the file will be shared.
- Readable and stable beats opaque: `lib:ada-lovelace`. Slugs from the label, lower case, ASCII, hyphens. Keep the source's own identifier when there is one (`lib:isbn-9780141439518`, an ORCID).
- Never put the file version or a date in the IRI of an entity.
- Link to the world with `schema:sameAs` or `owl:sameAs` to Wikidata, ORCID, ROR, GeoNames. Those links are what make the graph joinable.
- Avoid blank nodes for anything someone will query by name. Use them only for small structures that exist only inside their parent.

## Reuse vocabularies before you invent terms

Look for an existing class or property first, with `suggest_vocabulary(query)` on
the server (it searches Linked Open Vocabularies), or from memory:

| For | Use |
|---|---|
| names, labels | `rdfs:label` (always), `schema:name`, `skos:prefLabel`, `foaf:name` |
| people, organisations | `schema:Person`, `schema:Organization`, `foaf:Person`, `org:Organization` |
| works, articles, books | `schema:ScholarlyArticle`, `schema:Book`, `dcterms:title`, `dcterms:creator`, `bibo:` |
| places | `schema:Place`, `geo:` (WGS84 lat/long), GeoSPARQL `geo:asWKT` |
| dates | `xsd:date`, `xsd:dateTime`, `xsd:gYear` typed literals |
| categories and concept schemes | `skos:Concept`, `skos:broader`, `skos:inScheme` |
| datasets and provenance | `dcat:Dataset`, `dcterms:source`, `prov:wasDerivedFrom`, `prov:wasAttributedTo` |
| identifiers and equivalence | `schema:identifier`, `owl:sameAs`, `schema:sameAs` |
| class and property hierarchies | `rdfs:subClassOf`, `rdfs:subPropertyOf` (the engine's `reason=true` uses them) |

Only mint your own terms for what no vocabulary covers, and say how they relate
to the ones that exist: `my:Preprint rdfs:subClassOf schema:ScholarlyArticle`.
Declare each class and property you mint in the graph with a label and a comment.

## Typed literals

An untyped `"1843"` is a string. A typed one is a number or a date and sorts,
compares and aggregates as one.

| Value | Write |
|---|---|
| whole number | `"40"^^xsd:integer`, or just `40` in Turtle |
| decimal | `"3.14"^^xsd:decimal` |
| date | `"1815-12-10"^^xsd:date` |
| year only | `"1843"^^xsd:gYear` |
| timestamp | `"2026-11-28T09:30:00Z"^^xsd:dateTime` |
| text in a language | `"Genève"@fr` |
| true or false | `true` |

Pick one type per property and keep it. Mixed types in one column make
`ORDER BY` and `FILTER` unreliable.

## A small ontology (optional but worth it)

For a graph others will use, declare the model in the same file: the classes,
properties, their labels, comments, domains and ranges.

```turtle
@prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .
@prefix owl:  <http://www.w3.org/2002/07/owl#> .
@prefix xsd:  <http://www.w3.org/2001/XMLSchema#> .
@prefix lib:  <https://example.org/library/> .
@prefix schema: <https://schema.org/> .

lib:Preprint a owl:Class ;
    rdfs:subClassOf schema:ScholarlyArticle ;
    rdfs:label "Preprint" ;
    rdfs:comment "A paper shared before peer review." .

lib:reviewStatus a owl:DatatypeProperty ;
    rdfs:domain lib:Preprint ;
    rdfs:range xsd:string ;
    rdfs:label "review status" ;
    rdfs:comment "open, in review or accepted." .
```

On the server, run `check_ontology(ontology)` and fix what it reports (domains
over undeclared classes, dangling `subClassOf`, missing labels, a property typed
as both object and datatype, subclass cycles) until `ok` is true, then build.

## Checks before you build

- Every entity has an `rdf:type` and an `rdfs:label`.
- Every link points to an IRI that exists in the data (or a known external one).
- Dates and numbers are typed.
- No placeholder values (`N/A`, `-`, `0` for unknown). Leave the triple out.
- IRIs are unique per thing: two rows for one person are two people to the graph.
- The file parses. Locally, `rete validate data.ttl`; on the server, `build_rete` fails with the parse error and its position.

## A CSV to Turtle script

Plain Python, no libraries. It reads `papers.csv` with the columns `id`, `title`,
`year` and `authors` (names separated by `;`) and writes Turtle with people as
their own entities. Adjust names and types to your data.

```python
import csv, re

BASE = "https://example.org/library/"

def slug(text):
    return re.sub(r"[^a-z0-9]+", "-", text.lower()).strip("-")

def lit(text):
    return '"' + text.replace("\\", "\\\\").replace('"', '\\"').replace("\n", "\\n") + '"'

people, out = {}, []
with open("papers.csv", newline="", encoding="utf-8") as f:
    for row in csv.DictReader(f):
        paper = f"<{BASE}paper-{slug(row['id'])}>"
        out.append(f"{paper} a <https://schema.org/ScholarlyArticle> ;")
        out.append(f"    <http://www.w3.org/2000/01/rdf-schema#label> {lit(row['title'])} ;")
        if row["year"].strip():
            out.append(f'    <https://schema.org/datePublished> "{int(row["year"])}"^^<http://www.w3.org/2001/XMLSchema#gYear> ;')
        for name in filter(None, (n.strip() for n in row["authors"].split(";"))):
            people[slug(name)] = name
            out.append(f"    <https://schema.org/author> <{BASE}person-{slug(name)}> ;")
        out[-1] = out[-1][:-1] + "."          # the last line of each paper ends the statement

for key, name in people.items():
    out.append(f"<{BASE}person-{key}> a <https://schema.org/Person> ; <http://www.w3.org/2000/01/rdf-schema#label> {lit(name)} .")

open("papers.ttl", "w", encoding="utf-8").write("\n".join(out) + "\n")
print(len(people), "people")
```

Two traps in that script, both common: escaping quotes and backslashes in
literals (`lit`), and ending the last property of a subject with `.` instead of
`;`. Run `rete validate papers.ttl` (or build on the server) to catch either.
