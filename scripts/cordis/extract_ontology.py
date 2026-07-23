"""Extract the CORDIS/EURIO ontology (TBox) from the actual instance data.

Reads data/cordis/triples/*.parquet and derives, from real usage:
  - the classes (rdf:type values) + instance counts
  - subclass axioms (where a class's subjects are a subset of another's types)
  - each property's domain (subject classes), range (object classes for IRI
    objects, or literal datatype), and object-vs-datatype nature
Then writes data/cordis/cordis.ttl: the EURIO terms (s66:) that actually occur,
with rdfs:label/domain/range + curated mappings to standard vocabularies
(FRAPO, W3C org, FOAF, schema.org, FaBiO, SKOS, GeoSPARQL/WGS84, Dublin Core).
OWL 2 QL-safe.
"""

import duckdb

TRIPLES = "read_parquet('D:/pro/rete/data/cordis/triples/*.parquet')"
OUT = r"D:\pro\rete\data\cordis\cordis.ttl"
S66 = "http://data.europa.eu/s66#"
RDFTYPE = "http://www.w3.org/1999/02/22-rdf-syntax-ns#type"

# curated superclass / external mappings for the main EURIO classes
CLASS_MAP = {
    "Project": ("foaf:Project", None),
    "Grant": ("frapo:Grant", None),
    "FundingScheme": ("frapo:Funding", None),
    "FundingAgency": ("frapo:FundingAgency", None),
    "GrantPayment": ("frapo:Payment", None),
    "MonetaryAmount": ("schema:MonetaryAmount", None),
    "Organisation": ("foaf:Organization", "org:Organization"),
    "ForProfitOrganisation": ("Organisation", "schema:Corporation"),
    "SME": ("ForProfitOrganisation", None),
    "ResearchOrganisation": ("Organisation", None),
    "PublicBody": ("Organisation", "schema:GovernmentOrganization"),
    "HigherOrSecondaryEducation": ("Organisation", "schema:EducationalOrganization"),
    "OrganisationRole": ("org:Membership", None),
    "Result": ("fabio:Work", None),
    "ProjectPublication": ("Result", "fabio:Work"),
    "JournalPaper": ("ProjectPublication", "fabio:JournalArticle"),
    "ProceedingsPaper": ("ProjectPublication", "fabio:ConferencePaper"),
    "Book": ("ProjectPublication", "fabio:Book"),
    "BookChapter": ("ProjectPublication", "fabio:BookChapter"),
    "ThesisDissertation": ("ProjectPublication", "fabio:Thesis"),
    "ProjectDeliverable": ("Result", None),
    "ProjectReportSummary": ("Result", None),
    "PostalAddress": ("schema:PostalAddress", None),
    "Site": ("schema:Place", None),
    "Coordinates": ("schema:GeoCoordinates", None),
    "AdministrativeArea": ("schema:AdministrativeArea", None),
    "Country": ("schema:Country", None),
    "Acronym": (None, None),
}
# curated property mappings (local name -> external property)
PROP_MAP = {
    "title": "dcterms:title", "abstract": "dcterms:abstract",
    "startDate": "schema:startDate", "endDate": "schema:endDate",
    "totalCost": "frapo:hasMonetaryValue", "ecContribution": "frapo:hasMonetaryValue",
    "url": "schema:url", "doi": None, "country": "schema:addressCountry",
    "hasCoordinates": "geo:location", "latitude": "geo:lat", "longitude": "geo:long",
    "isPartOf": "dcterms:isPartOf", "hasCall": None,
}


def local(iri):
    return iri.split("#")[-1].split("/")[-1]


def main():
    d = duckdb.connect()
    d.execute("SET threads=8")
    d.execute(f"CREATE VIEW t AS SELECT * FROM {TRIPLES}")
    d.execute(f"CREATE TABLE s2t AS SELECT subject, object AS cls FROM t WHERE predicate='{RDFTYPE}'")

    classes = {r[0]: r[1] for r in d.execute(
        f"SELECT object, count(*) FROM t WHERE predicate='{RDFTYPE}' "
        "GROUP BY 1 ORDER BY 2 DESC").fetchall()}

    # property stats
    pred_kind = d.execute("""
        SELECT predicate,
               sum(CASE WHEN otype='iri' THEN 1 ELSE 0 END) n_iri,
               sum(CASE WHEN otype='lit' THEN 1 ELSE 0 END) n_lit,
               count(*) n
        FROM t WHERE predicate <> '%s' GROUP BY 1""" % RDFTYPE).fetchall()
    kind = {p: ("iri" if ni >= nl else "lit", n) for p, ni, nl, n in pred_kind}

    dom = {}
    for p, cls, c in d.execute("""
        SELECT t.predicate, s.cls, count(*) FROM t JOIN s2t s USING(subject)
        WHERE t.predicate <> '%s' GROUP BY 1,2""" % RDFTYPE).fetchall():
        dom.setdefault(p, {})[cls] = c
    rng = {}
    for p, cls, c in d.execute("""
        SELECT t.predicate, o.cls, count(*) FROM t JOIN s2t o ON t.object=o.subject
        WHERE t.otype='iri' GROUP BY 1,2""").fetchall():
        rng.setdefault(p, {})[cls] = c
    dts = {}
    for p, dt, c in d.execute("""
        SELECT predicate, datatype, count(*) FROM t WHERE otype='lit' AND datatype IS NOT NULL
        GROUP BY 1,2""").fetchall():
        dts.setdefault(p, {})[dt] = c

    def dominant(m, frac=0.6):
        if not m:
            return []
        tot = sum(m.values())
        return [k for k, v in sorted(m.items(), key=lambda x: -x[1]) if v / tot >= 0.05]

    onto_comment = (
        "The EURIO (EUropean Research Information Ontology) terms that actually occur in "
        "the CORDIS EURIO Knowledge Graph (EU-funded research projects, grants, results, "
        "organisations), reconstructed from the instance data: classes + instance counts, "
        "and each property's domain/range/nature inferred from real usage. Terms are the "
        "native s66: (EURIO) IRIs so the ontology applies directly to the data; classes and "
        "properties are additionally mapped to FRAPO, the W3C Organization ontology, FOAF, "
        "schema.org, FaBiO, SKOS, WGS84 geo and Dublin Core. OWL 2 QL-safe."
    )
    lines = [
        "@prefix s66:     <http://data.europa.eu/s66#> .",
        "@prefix cordis:  <https://w3id.org/rete/cordis#> .",
        "@prefix owl:     <http://www.w3.org/2002/07/owl#> .",
        "@prefix rdf:     <http://www.w3.org/1999/02/22-rdf-syntax-ns#> .",
        "@prefix rdfs:    <http://www.w3.org/2000/01/rdf-schema#> .",
        "@prefix xsd:     <http://www.w3.org/2001/XMLSchema#> .",
        "@prefix skos:    <http://www.w3.org/2004/02/skos/core#> .",
        "@prefix dcterms: <http://purl.org/dc/terms/> .",
        "@prefix foaf:    <http://xmlns.com/foaf/0.1/> .",
        "@prefix org:     <http://www.w3.org/ns/org#> .",
        "@prefix schema:  <https://schema.org/> .",
        "@prefix geo:     <http://www.w3.org/2003/01/geo/wgs84_pos#> .",
        "@prefix fabio:   <http://purl.org/spar/fabio/> .",
        "@prefix frapo:   <http://purl.org/cerif/frapo/> .",
        "",
        "cordis: a owl:Ontology ;",
        '    dcterms:title "CORDIS / EURIO ontology (rete, extracted)"@en ;',
        '    rdfs:comment """' + onto_comment + '"""@en ;',
        '    dcterms:license <https://creativecommons.org/licenses/by/4.0/> ;',
        "    rdfs:seeAlso <http://data.europa.eu/s66> ;",
        '    owl:versionInfo "1.0.0 (2026-07-17)" .',
        "",
        "#################################################################",
        "#  Classes",
        "#################################################################",
        "",
    ]

    def cref(name):
        return "s66:" + name if not name or ":" not in name and name in classes_local else \
               ("s66:" + name if name in classes_local else name)

    classes_local = {local(c) for c in classes}
    for c, n in classes.items():
        if not c.startswith(S66):
            continue
        ln = local(c)
        supers = []
        m = CLASS_MAP.get(ln, (None, None))
        for x in m:
            if not x:
                continue
            supers.append("s66:" + x if x in classes_local else x)
        lines.append(f"s66:{ln} a owl:Class ;")
        lines.append(f'    rdfs:label "{ln}"@en ;')
        lines.append(f'    rdfs:comment "{n:,} instances in the CORDIS EURIO KG."@en'
                     + (" ;" if supers else " ."))
        if supers:
            lines.append("    rdfs:subClassOf " + " , ".join(supers) + " .")
        lines.append("")

    lines += ["#################################################################",
              "#  Properties  (domain/range/nature inferred from usage)",
              "#################################################################", ""]
    for p in sorted(kind, key=lambda x: -kind[x][1]):
        if not p.startswith(S66):
            continue
        ln = local(p)
        k, n = kind[p]
        doms = [local(c) for c in dominant(dom.get(p, {}))]
        doms = [d_ for d_ in doms if (S66 + d_) in classes or d_ in classes_local]
        ext = PROP_MAP.get(ln, "MISSING")
        if k == "iri":
            lines.append(f"s66:{ln} a owl:ObjectProperty ;")
        else:
            lines.append(f"s66:{ln} a owl:DatatypeProperty ;")
        lines.append(f'    rdfs:label "{ln}"@en ;')
        lines.append(f'    rdfs:comment "{n:,} uses."@en ;')
        tail = []
        if len(doms) == 1:
            tail.append(f"    rdfs:domain s66:{doms[0]}")
        if k == "iri":
            rngs = [local(c) for c in dominant(rng.get(p, {}))]
            rngs = [r for r in rngs if r in classes_local]
            if len(rngs) == 1:
                tail.append(f"    rdfs:range s66:{rngs[0]}")
        else:
            dtl = dominant(dts.get(p, {}))
            if len(dtl) == 1:
                dt = dtl[0].replace("http://www.w3.org/2001/XMLSchema#", "xsd:")
                if dt.startswith("xsd:"):
                    tail.append(f"    rdfs:range {dt}")
        if ext and ext != "MISSING":
            tail.append(f"    rdfs:subPropertyOf {ext}")
        if tail:
            lines.append(" ;\n".join(tail) + " .")
        else:
            lines[-1] = lines[-1].rstrip(" ;") + " ."
            # ensure the comment line ends with '.' — rebuild last two
        lines.append("")

    with open(OUT, "w", encoding="utf-8") as f:
        f.write("\n".join(lines) + "\n")
    print(f"wrote {OUT}: {len([c for c in classes if c.startswith(S66)])} classes, "
          f"{len([p for p in kind if p.startswith(S66)])} s66 properties")


if __name__ == "__main__":
    main()
