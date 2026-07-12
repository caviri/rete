//! End-to-end integration tests: build a real `.rete` image through the public
//! API and run a broad battery of SPARQL queries against it, exercising how the
//! features combine. Complements the per-feature unit tests in `src/sparql.rs`.

use rete_core::{
    build_pyramid_meta, eval_query, eval_sparql, eval_sparql_reasoned, write_dataset, write_file,
    DictionaryBuilder, GraphIndexBuilder, QueryOutput, Rete, DEFAULT_TILE_BUDGET,
};

const XSD_INT: &str = "<http://www.w3.org/2001/XMLSchema#integer>";

/// Build a `.rete` image (with pyramid) from `(s, p, o)` term-token triples.
fn build(triples: &[(&str, &str, &str)]) -> Vec<u8> {
    let mut db = DictionaryBuilder::new();
    for (s, p, o) in triples {
        db.observe(s, p, o);
    }
    let dict = db.build();
    let ids: Vec<_> = triples
        .iter()
        .map(|(s, p, o)| dict.encode(s, p, o).expect("known term"))
        .collect();
    let mut ib = GraphIndexBuilder::new();
    for &t in &ids {
        ib.push(t);
    }
    let (meta, levels) = build_pyramid_meta(&dict, &ids, DEFAULT_TILE_BUDGET);
    write_file(&dict, &ib.build(), false, &meta, levels)
}

/// A small social dataset: 5 people with names/ages/cities and `knows` edges.
fn dataset() -> Vec<u8> {
    let int = |n: &str| format!("\"{n}\"^^{XSD_INT}");
    let t: Vec<(String, String, String)> = vec![
        ("Alice", "name", "\"Alice\""),
        ("Bob", "name", "\"Bob\""),
        ("Carol", "name", "\"Carol\""),
        ("Dave", "name", "\"Dave\""),
        ("Eve", "name", "\"Eve\""),
        ("Alice", "age", &int("30")),
        ("Bob", "age", &int("25")),
        ("Carol", "age", &int("35")),
        ("Dave", "age", &int("40")),
        ("Alice", "city", "City:NYC"),
        ("Bob", "city", "City:LA"),
        ("Carol", "city", "City:NYC"),
        // knows chain: Alice -> Bob -> Carol -> Dave -> Eve, plus Alice -> Carol.
        ("Alice", "knows", "Bob"),
        ("Bob", "knows", "Carol"),
        ("Carol", "knows", "Dave"),
        ("Dave", "knows", "Eve"),
        ("Alice", "knows", "Carol"),
    ]
    .into_iter()
    .map(|(s, p, o)| (iri(s), iri(p), term(o)))
    .collect();
    let refs: Vec<(&str, &str, &str)> = t
        .iter()
        .map(|(s, p, o)| (s.as_str(), p.as_str(), o.as_str()))
        .collect();
    build(&refs)
}

fn iri(s: &str) -> String {
    format!("<http://ex/{s}>")
}
/// Object term: literals (start with `"`) pass through; `City:X` and bare names
/// become IRIs.
fn term(o: &str) -> String {
    if o.starts_with('"') {
        o.to_string()
    } else if let Some(c) = o.strip_prefix("City:") {
        format!("<http://ex/city/{c}>")
    } else {
        iri(o)
    }
}

const PREFIX: &str = "PREFIX ex: <http://ex/> ";

/// Run a SELECT and return values of `var`, sorted.
fn col(rete: &Rete, q: &str, var: &str) -> Vec<String> {
    let (_, sols) = eval_sparql(rete, &format!("{PREFIX}{q}")).unwrap();
    let mut v: Vec<String> = sols.iter().filter_map(|b| b.get(var).cloned()).collect();
    v.sort();
    v
}

#[test]
fn rdf_star_ingest_header_flag_and_concrete_query() {
    use rete_core::ingest::{assemble_dataset, parse};
    let rdf = "<http://www.w3.org/1999/02/22-rdf-syntax-ns#type>";
    // A sighting typed, then two annotations ON that typing statement (RDF-star).
    let nt = format!(
        "<http://ex/occ1> {rdf} <http://ex/Swallow> .\n\
         << <http://ex/occ1> {rdf} <http://ex/Swallow> >> <http://ex/recordedBy> \"J. Smith\" .\n\
         << <http://ex/occ1> {rdf} <http://ex/Swallow> >> <http://ex/count> \"5\" .\n"
    );
    let quads: Vec<_> = parse(&nt)
        .unwrap()
        .into_iter()
        .map(|(s, p, o)| (s, p, o, None))
        .collect();
    let (image, _) = assemble_dataset(quads, &[]);
    let rete = Rete::open(&image).unwrap();

    // The header records that the file contains quoted triples (the RDF/RDF-star
    // compatibility signal — a plain-RDF consumer reads it without scanning).
    assert!(rete.header().has_quoted_triples());

    // SPARQL-star: look up an annotation on a KNOWN (concrete) quoted triple.
    let who = col(
        &rete,
        &format!(
            "SELECT ?who WHERE {{ << <http://ex/occ1> {rdf} <http://ex/Swallow> >> \
             <http://ex/recordedBy> ?who }}"
        ),
        "who",
    );
    assert_eq!(who, vec!["\"J. Smith\"".to_string()]);

    // The quoted triple is itself a first-class term: it binds as a subject and
    // round-trips in its canonical `<< s p o >>` surface.
    let subj = col(&rete, "SELECT ?s WHERE { ?s <http://ex/count> \"5\" }", "s");
    assert_eq!(subj.len(), 1);
    assert!(subj[0].starts_with("<<") && subj[0].ends_with(">>"));

    // SPARQL-star builtins: isTRIPLE filters quoted triples; SUBJECT/OBJECT
    // decompose them (so inner-variable queries work via the builtins).
    let annotated_subjects = col(
        &rete,
        "SELECT ?s WHERE { ?qt <http://ex/recordedBy> ?who FILTER(isTRIPLE(?qt)) \
         BIND(SUBJECT(?qt) AS ?s) }",
        "s",
    );
    assert_eq!(annotated_subjects, vec!["<http://ex/occ1>".to_string()]);

    // TRIPLE(s, p, o) constructs a quoted triple in canonical surface.
    let built = col(
        &rete,
        "SELECT ?t WHERE { <http://ex/occ1> ?p ?o \
         BIND(TRIPLE(<http://ex/occ1>, ?p, ?o) AS ?t) }",
        "t",
    );
    assert_eq!(
        built,
        vec![format!("<<<http://ex/occ1> {rdf} <http://ex/Swallow>>>")]
    );
}

#[test]
fn rdf_star_quoted_triple_patterns() {
    use rete_core::ingest::{assemble_dataset, parse};
    let rdf = "<http://www.w3.org/1999/02/22-rdf-syntax-ns#type>";
    let nt = format!(
        "<http://ex/occ1> {rdf} <http://ex/Swallow> .\n\
         << <http://ex/occ1> {rdf} <http://ex/Swallow> >> <http://ex/recordedBy> \"J. Smith\" .\n\
         <http://ex/occ2> {rdf} <http://ex/Robin> .\n\
         << <http://ex/occ2> {rdf} <http://ex/Robin> >> <http://ex/recordedBy> \"A. Jones\" .\n\
         <http://ex/occ1> <http://ex/place> \"Wetland\" .\n"
    );
    let quads: Vec<_> = parse(&nt)
        .unwrap()
        .into_iter()
        .map(|(s, p, o)| (s, p, o, None))
        .collect();
    let (image, _) = assemble_dataset(quads, &[]);
    let rete = Rete::open(&image).unwrap();

    // Pattern sugar, EXTRACTION: inner variables are bound from the matched
    // quoted triple.
    let species = col(
        &rete,
        &format!("SELECT ?o WHERE {{ << ?s {rdf} ?o >> <http://ex/recordedBy> ?who }}"),
        "o",
    );
    assert_eq!(
        species,
        vec![
            "<http://ex/Robin>".to_string(),
            "<http://ex/Swallow>".to_string()
        ]
    );

    // A CONCRETE inner term filters the match.
    let who = col(
        &rete,
        &format!(
            "SELECT ?who WHERE {{ << ?s {rdf} <http://ex/Swallow> >> <http://ex/recordedBy> ?who }}"
        ),
        "who",
    );
    assert_eq!(who, vec!["\"J. Smith\"".to_string()]);

    // JOIN: `?occ` is bound by a regular pattern AND appears inside the quoted
    // triple, so the two must unify (only occ1 has a place).
    let joined = col(
        &rete,
        &format!(
            "SELECT ?who WHERE {{ ?occ <http://ex/place> ?p . \
             << ?occ {rdf} ?sp >> <http://ex/recordedBy> ?who }}"
        ),
        "who",
    );
    assert_eq!(joined, vec!["\"J. Smith\"".to_string()]);
}

#[test]
fn rdf_star_multiple_annotations_on_one_quoted_triple() {
    // Two annotations on ONE inner-variable quoted triple
    // (`<< ?s p ?o >> :a ?x ; :b ?y`) must JOIN on the shared statement — the
    // natural provenance shape (a name annotated with BOTH source and date).
    // Regression: the rewrite used to mint a distinct fresh var per occurrence,
    // so the two BGP patterns shared no variable and Cartesian-multiplied every
    // annotated triple against every other (correct on tiny data, but a hang on
    // real data — this locks in the correctness half).
    use rete_core::ingest::{assemble_dataset, parse};
    let rdf = "<http://www.w3.org/1999/02/22-rdf-syntax-ns#type>";
    let nt = format!(
        "<http://ex/occ1> {rdf} <http://ex/Swallow> .\n\
         << <http://ex/occ1> {rdf} <http://ex/Swallow> >> <http://ex/by> \"Smith\" .\n\
         << <http://ex/occ1> {rdf} <http://ex/Swallow> >> <http://ex/n> \"5\" .\n\
         <http://ex/occ2> {rdf} <http://ex/Robin> .\n\
         << <http://ex/occ2> {rdf} <http://ex/Robin> >> <http://ex/by> \"Jones\" .\n\
         <http://ex/occ3> {rdf} <http://ex/Swallow> .\n\
         << <http://ex/occ3> {rdf} <http://ex/Swallow> >> <http://ex/by> \"Lee\" .\n\
         << <http://ex/occ3> {rdf} <http://ex/Swallow> >> <http://ex/n> \"3\" .\n"
    );
    let quads: Vec<_> = parse(&nt)
        .unwrap()
        .into_iter()
        .map(|(s, p, o)| (s, p, o, None))
        .collect();
    let (image, _) = assemble_dataset(quads, &[]);
    let rete = Rete::open(&image).unwrap();

    // occ1 and occ3 have BOTH annotations; occ2 has only :by, so it is excluded.
    let who = col(
        &rete,
        &format!(
            "SELECT ?who WHERE {{ << ?s {rdf} ?o >> <http://ex/by> ?who ; <http://ex/n> ?count }}"
        ),
        "who",
    );
    assert_eq!(who, vec!["\"Lee\"".to_string(), "\"Smith\"".to_string()]);

    // And the second annotation's value binds correctly (not crossed).
    let counts = col(
        &rete,
        &format!(
            "SELECT ?count WHERE {{ << ?s {rdf} ?o >> <http://ex/by> ?who ; <http://ex/n> ?count }}"
        ),
        "count",
    );
    assert_eq!(counts, vec!["\"3\"".to_string(), "\"5\"".to_string()]);
}

#[test]
fn owl_ql_subclass_reasoning() {
    // OWL 2 QL Stage 1a: `?x a C` is entailed by any `?x a D` with D ⊑* C. With
    // reasoning ON the query is rewritten to walk `subClassOf*` over the RAW data;
    // OFF, it is the plain (direct-type-only) query. Sound + complete for subClassOf.
    let ty = "<http://www.w3.org/1999/02/22-rdf-syntax-ns#type>";
    let sub = "<http://www.w3.org/2000/01/rdf-schema#subClassOf>";
    let mk = |n: &str| format!("<http://ex/{n}>");
    // Sparrow ⊑ Passerine ⊑ Bird ; Eagle ⊑ Raptor ⊑ Bird ; three typed individuals.
    let t: Vec<(String, String, String)> = vec![
        (mk("Sparrow"), sub.to_string(), mk("Passerine")),
        (mk("Passerine"), sub.to_string(), mk("Bird")),
        (mk("Eagle"), sub.to_string(), mk("Raptor")),
        (mk("Raptor"), sub.to_string(), mk("Bird")),
        (mk("occ1"), ty.to_string(), mk("Sparrow")),
        (mk("occ2"), ty.to_string(), mk("Eagle")),
        (mk("occ3"), ty.to_string(), mk("Bird")),
    ];
    let refs: Vec<(&str, &str, &str)> = t
        .iter()
        .map(|(s, p, o)| (s.as_str(), p.as_str(), o.as_str()))
        .collect();
    let rete = Rete::open(&build(&refs)).unwrap();
    let sorted = |sols: Vec<rete_core::Binding>| {
        let mut v: Vec<String> = sols.iter().filter_map(|b| b.get("x").cloned()).collect();
        v.sort();
        v
    };

    // `?x a :Bird`: plain = the direct instance only; reasoned = subclasses too.
    let q = format!("{PREFIX}SELECT ?x WHERE {{ ?x a ex:Bird }}");
    assert_eq!(sorted(eval_sparql(&rete, &q).unwrap().1), vec![mk("occ3")]);
    assert_eq!(
        sorted(eval_sparql_reasoned(&rete, &q).unwrap().1),
        vec![mk("occ1"), mk("occ2"), mk("occ3")]
    );

    // A mid-level class: `?x a :Passerine` reasons to occ1 (Sparrow ⊑ Passerine).
    let qp = format!("{PREFIX}SELECT ?x WHERE {{ ?x a ex:Passerine }}");
    assert!(eval_sparql(&rete, &qp).unwrap().1.is_empty());
    assert_eq!(
        sorted(eval_sparql_reasoned(&rete, &qp).unwrap().1),
        vec![mk("occ1")]
    );

    // Reasoning also composes with other patterns: a leaf class is unchanged.
    let ql = format!("{PREFIX}SELECT ?x WHERE {{ ?x a ex:Sparrow }}");
    assert_eq!(
        sorted(eval_sparql_reasoned(&rete, &ql).unwrap().1),
        vec![mk("occ1")]
    );
}

#[test]
fn owl_ql_subproperty_reasoning() {
    // OWL 2 QL Stage 1a: a role atom `?x P ?y` is entailed by any `?x Q ?y` with
    // Q rdfs:subPropertyOf* P. Rewritten to walk subPropertyOf* over the raw data.
    let subp = "<http://www.w3.org/2000/01/rdf-schema#subPropertyOf>";
    let mk = |n: &str| format!("<http://ex/{n}>");
    let t: Vec<(String, String, String)> = vec![
        (mk("hasFather"), subp.to_string(), mk("hasParent")),
        (mk("hasMother"), subp.to_string(), mk("hasParent")),
        (mk("a"), mk("hasFather"), mk("b")),
        (mk("c"), mk("hasMother"), mk("d")),
        (mk("e"), mk("hasParent"), mk("f")), // a direct hasParent assertion
    ];
    let refs: Vec<(&str, &str, &str)> = t
        .iter()
        .map(|(s, p, o)| (s.as_str(), p.as_str(), o.as_str()))
        .collect();
    let rete = Rete::open(&build(&refs)).unwrap();
    let pairs = |sols: Vec<rete_core::Binding>| {
        let mut v: Vec<String> = sols
            .iter()
            .map(|b| format!("{}->{}", b.get("x").unwrap(), b.get("y").unwrap()))
            .collect();
        v.sort();
        v
    };

    // `?x :hasParent ?y`: plain = only the direct assertion; reasoned = the
    // hasFather / hasMother edges too.
    let q = format!("{PREFIX}SELECT ?x ?y WHERE {{ ?x ex:hasParent ?y }}");
    assert_eq!(
        pairs(eval_sparql(&rete, &q).unwrap().1),
        vec!["<http://ex/e>-><http://ex/f>"]
    );
    assert_eq!(
        pairs(eval_sparql_reasoned(&rete, &q).unwrap().1),
        vec![
            "<http://ex/a>-><http://ex/b>",
            "<http://ex/c>-><http://ex/d>",
            "<http://ex/e>-><http://ex/f>",
        ]
    );

    // A property with no subproperties is left exactly as written (gate skips it).
    let ql = format!("{PREFIX}SELECT ?x ?y WHERE {{ ?x ex:hasFather ?y }}");
    assert_eq!(
        pairs(eval_sparql_reasoned(&rete, &ql).unwrap().1),
        vec!["<http://ex/a>-><http://ex/b>"]
    );
}

#[test]
fn owl_ql_domain_range_reasoning() {
    // OWL 2 QL Stage 1b: a subject of a property whose DOMAIN is `⊑* C` is a C;
    // an object of a property whose RANGE is `⊑* C` is a C. Rewritten to patterns
    // over the raw data (no materialization). Composes with subClassOf.
    let dom = "<http://www.w3.org/2000/01/rdf-schema#domain>";
    let rng = "<http://www.w3.org/2000/01/rdf-schema#range>";
    let sub = "<http://www.w3.org/2000/01/rdf-schema#subClassOf>";
    let mk = |n: &str| format!("<http://ex/{n}>");
    let t: Vec<(String, String, String)> = vec![
        (mk("worksAt"), dom.to_string(), mk("Employee")),
        (mk("hasCapital"), rng.to_string(), mk("City")),
        (mk("Employee"), sub.to_string(), mk("Person")), // Employee ⊑ Person
        (mk("alice"), mk("worksAt"), mk("acme")),
        (mk("france"), mk("hasCapital"), mk("paris")),
    ];
    let refs: Vec<(&str, &str, &str)> = t
        .iter()
        .map(|(s, p, o)| (s.as_str(), p.as_str(), o.as_str()))
        .collect();
    let rete = Rete::open(&build(&refs)).unwrap();
    let xs = |sols: Vec<rete_core::Binding>| {
        let mut v: Vec<String> = sols.iter().filter_map(|b| b.get("x").cloned()).collect();
        v.sort();
        v.dedup();
        v
    };

    // Domain: alice worksAt acme ⇒ alice a :Employee (nothing asserts it directly).
    let qe = format!("{PREFIX}SELECT ?x WHERE {{ ?x a ex:Employee }}");
    assert!(eval_sparql(&rete, &qe).unwrap().1.is_empty());
    assert_eq!(
        xs(eval_sparql_reasoned(&rete, &qe).unwrap().1),
        vec![mk("alice")]
    );

    // Range: france hasCapital paris ⇒ paris a :City.
    let qc = format!("{PREFIX}SELECT ?x WHERE {{ ?x a ex:City }}");
    assert!(eval_sparql(&rete, &qc).unwrap().1.is_empty());
    assert_eq!(
        xs(eval_sparql_reasoned(&rete, &qc).unwrap().1),
        vec![mk("paris")]
    );

    // Composition: alice is an Employee (domain) and Employee ⊑ Person ⇒ Person.
    let qp = format!("{PREFIX}SELECT ?x WHERE {{ ?x a ex:Person }}");
    assert_eq!(
        xs(eval_sparql_reasoned(&rete, &qp).unwrap().1),
        vec![mk("alice")]
    );
}

#[test]
fn owl_ql_inverse_reasoning() {
    // OWL 2 QL Stage 2: a role atom `?x P ?y` is also entailed by `?y Q ?x` when
    // Q owl:inverseOf P (either declared direction), rewritten as a UNION branch.
    let inv = "<http://www.w3.org/2002/07/owl#inverseOf>";
    let mk = |n: &str| format!("<http://ex/{n}>");
    let t: Vec<(String, String, String)> = vec![
        (mk("hasChild"), inv.to_string(), mk("hasParent")),
        (mk("alice"), mk("hasChild"), mk("bob")), // ⇒ bob hasParent alice
        (mk("carol"), mk("hasParent"), mk("dave")), // a direct hasParent
    ];
    let refs: Vec<(&str, &str, &str)> = t
        .iter()
        .map(|(s, p, o)| (s.as_str(), p.as_str(), o.as_str()))
        .collect();
    let rete = Rete::open(&build(&refs)).unwrap();
    let pairs = |sols: Vec<rete_core::Binding>| {
        let mut v: Vec<String> = sols
            .iter()
            .map(|b| format!("{}->{}", b.get("x").unwrap(), b.get("y").unwrap()))
            .collect();
        v.sort();
        v.dedup();
        v
    };

    // `?x :hasParent ?y`: plain = the direct edge; reasoned = the inverse of
    // hasChild too (alice hasChild bob ⇒ bob hasParent alice).
    let q = format!("{PREFIX}SELECT ?x ?y WHERE {{ ?x ex:hasParent ?y }}");
    assert_eq!(
        pairs(eval_sparql(&rete, &q).unwrap().1),
        vec!["<http://ex/carol>-><http://ex/dave>"]
    );
    assert_eq!(
        pairs(eval_sparql_reasoned(&rete, &q).unwrap().1),
        vec![
            "<http://ex/bob>-><http://ex/alice>",
            "<http://ex/carol>-><http://ex/dave>",
        ]
    );

    // And the other direction: `?x :hasChild ?y` reasons over hasParent's inverse
    // (carol hasParent dave ⇒ dave hasChild carol), plus the direct edge.
    let qc = format!("{PREFIX}SELECT ?x ?y WHERE {{ ?x ex:hasChild ?y }}");
    assert_eq!(
        pairs(eval_sparql_reasoned(&rete, &qc).unwrap().1),
        vec![
            "<http://ex/alice>-><http://ex/bob>",
            "<http://ex/dave>-><http://ex/carol>",
        ]
    );
}

#[test]
fn owl_ql_existential_reasoning() {
    // OWL 2 QL Stage 2: `A ⊑ ∃P` (someValuesFrom). A query `?x P ?y` with `?y`
    // PURELY existential (occurs once, not returned) is entailed for every `?x`
    // that is (transitively) an A — the anonymous successor satisfies it. Must be
    // SOUND: it may NOT fire when `?y` is projected or shared elsewhere.
    let ty = "<http://www.w3.org/1999/02/22-rdf-syntax-ns#type>";
    let sub = "<http://www.w3.org/2000/01/rdf-schema#subClassOf>";
    let onp = "<http://www.w3.org/2002/07/owl#onProperty>";
    let svf = "<http://www.w3.org/2002/07/owl#someValuesFrom>";
    let mk = |n: &str| format!("<http://ex/{n}>");
    // Parent ⊑ ∃hasChild.Person ; alice a Parent (no ground child) ; bob hasChild carol.
    let t: Vec<(String, String, String)> = vec![
        (mk("Parent"), sub.to_string(), mk("R1")),
        (mk("R1"), onp.to_string(), mk("hasChild")),
        (mk("R1"), svf.to_string(), mk("Person")),
        (mk("alice"), ty.to_string(), mk("Parent")),
        (mk("bob"), mk("hasChild"), mk("carol")),
        (mk("carol"), ty.to_string(), mk("Person")),
    ];
    let refs: Vec<(&str, &str, &str)> = t
        .iter()
        .map(|(s, p, o)| (s.as_str(), p.as_str(), o.as_str()))
        .collect();
    let rete = Rete::open(&build(&refs)).unwrap();
    let xs = |sols: Vec<rete_core::Binding>| {
        let mut v: Vec<String> = sols.iter().filter_map(|b| b.get("x").cloned()).collect();
        v.sort();
        v.dedup();
        v
    };

    // Existential object: plain = only the ground edge; reasoned = alice too
    // (Parent ⊑ ∃hasChild), even with no ground child.
    let q = format!("{PREFIX}SELECT ?x WHERE {{ ?x ex:hasChild ?y }}");
    assert_eq!(xs(eval_sparql(&rete, &q).unwrap().1), vec![mk("bob")]);
    assert_eq!(
        xs(eval_sparql_reasoned(&rete, &q).unwrap().1),
        vec![mk("alice"), mk("bob")]
    );

    // SOUNDNESS 1 — `?y` is PROJECTED: the anonymous successor can't be returned,
    // so alice must NOT appear (only the ground pair).
    let qp = format!("{PREFIX}SELECT ?x ?y WHERE {{ ?x ex:hasChild ?y }}");
    assert_eq!(
        xs(eval_sparql_reasoned(&rete, &qp).unwrap().1),
        vec![mk("bob")]
    );

    // SOUNDNESS 2 — `?y` is SHARED (used in another atom): the anonymous successor
    // can't satisfy `?y a :Person`, so alice must NOT appear.
    let qs = format!("{PREFIX}SELECT ?x WHERE {{ ?x ex:hasChild ?y . ?y a ex:Person }}");
    assert_eq!(
        xs(eval_sparql_reasoned(&rete, &qs).unwrap().1),
        vec![mk("bob")]
    );
}

#[test]
fn owl_ql_inverse_existential_reasoning() {
    // OWL 2 QL: inverse existential `A ⊑ ∃P⁻`. With `hasChild owl:inverseOf
    // hasParent` and `Parent ⊑ ∃hasChild`, a Parent is (anonymously) someone's
    // parent, so `?y hasParent ?x` (subject `?y` existential) returns Parents.
    let ty = "<http://www.w3.org/1999/02/22-rdf-syntax-ns#type>";
    let sub = "<http://www.w3.org/2000/01/rdf-schema#subClassOf>";
    let inv = "<http://www.w3.org/2002/07/owl#inverseOf>";
    let onp = "<http://www.w3.org/2002/07/owl#onProperty>";
    let svf = "<http://www.w3.org/2002/07/owl#someValuesFrom>";
    let mk = |n: &str| format!("<http://ex/{n}>");
    let t: Vec<(String, String, String)> = vec![
        (mk("hasChild"), inv.to_string(), mk("hasParent")),
        (mk("Parent"), sub.to_string(), mk("R")),
        (mk("R"), onp.to_string(), mk("hasChild")),
        (mk("R"), svf.to_string(), mk("Person")),
        (mk("alice"), ty.to_string(), mk("Parent")),
        (mk("bob"), mk("hasParent"), mk("carol")), // a ground hasParent
    ];
    let refs: Vec<(&str, &str, &str)> = t
        .iter()
        .map(|(s, p, o)| (s.as_str(), p.as_str(), o.as_str()))
        .collect();
    let rete = Rete::open(&build(&refs)).unwrap();
    let xs = |sols: Vec<rete_core::Binding>| {
        let mut v: Vec<String> = sols.iter().filter_map(|b| b.get("x").cloned()).collect();
        v.sort();
        v.dedup();
        v
    };

    // `?y hasParent ?x` with `?y` existential: plain = the ground object; reasoned
    // = alice too (Parent ⊑ ∃hasChild ≡ ∃hasParent⁻).
    let q = format!("{PREFIX}SELECT ?x WHERE {{ ?y ex:hasParent ?x }}");
    assert_eq!(xs(eval_sparql(&rete, &q).unwrap().1), vec![mk("carol")]);
    assert_eq!(
        xs(eval_sparql_reasoned(&rete, &q).unwrap().1),
        vec![mk("alice"), mk("carol")]
    );

    // SOUNDNESS — `?y` projected: the anonymous child can't be returned.
    let qp = format!("{PREFIX}SELECT ?x ?y WHERE {{ ?y ex:hasParent ?x }}");
    assert_eq!(
        xs(eval_sparql_reasoned(&rete, &qp).unwrap().1),
        vec![mk("carol")]
    );
}

#[test]
fn owl_ql_domain_subproperty_composition() {
    // OWL 2 QL: domain composes with subPropertyOf. `hasSalary rdfs:domain
    // Employee` and `hasBonus ⊑ hasSalary`, so a subject of hasBonus (a
    // subproperty) is also an Employee.
    let dom = "<http://www.w3.org/2000/01/rdf-schema#domain>";
    let subp = "<http://www.w3.org/2000/01/rdf-schema#subPropertyOf>";
    let mk = |n: &str| format!("<http://ex/{n}>");
    let t: Vec<(String, String, String)> = vec![
        (mk("hasSalary"), dom.to_string(), mk("Employee")),
        (mk("hasBonus"), subp.to_string(), mk("hasSalary")),
        (mk("alice"), mk("hasBonus"), mk("v1000")),
        (mk("bob"), mk("hasSalary"), mk("v2000")),
    ];
    let refs: Vec<(&str, &str, &str)> = t
        .iter()
        .map(|(s, p, o)| (s.as_str(), p.as_str(), o.as_str()))
        .collect();
    let rete = Rete::open(&build(&refs)).unwrap();
    let xs = |sols: Vec<rete_core::Binding>| {
        let mut v: Vec<String> = sols.iter().filter_map(|b| b.get("x").cloned()).collect();
        v.sort();
        v.dedup();
        v
    };

    // `?x a :Employee`: reasoned finds bob (direct hasSalary) AND alice, whose
    // hasBonus is a subproperty of the domain-declared hasSalary.
    let q = format!("{PREFIX}SELECT ?x WHERE {{ ?x a ex:Employee }}");
    assert!(eval_sparql(&rete, &q).unwrap().1.is_empty());
    assert_eq!(
        xs(eval_sparql_reasoned(&rete, &q).unwrap().1),
        vec![mk("alice"), mk("bob")]
    );
}

#[test]
fn rdf12_base_direction_and_version() {
    // RDF 1.2: a base-direction language string `"x"@lang--dir` has datatype
    // rdf:dirLangString and LANG() = the language only. SPARQL 1.2: a leading
    // VERSION declaration is accepted (and dropped). Both without a crate swap.
    let dir = "http://www.w3.org/1999/02/22-rdf-syntax-ns#dirLangString";
    let plain = "http://www.w3.org/1999/02/22-rdf-syntax-ns#langString";
    let refs: Vec<(&str, &str, &str)> = vec![
        ("<http://ex/s>", "<http://ex/a>", "\"hello\"@en--ltr"), // directional
        ("<http://ex/s>", "<http://ex/b>", "\"hi\"@en"),         // plain lang string
    ];
    let rete = Rete::open(&build(&refs)).unwrap();

    // LANG strips the base direction: "hello"@en--ltr → "en".
    assert_eq!(
        col(
            &rete,
            "SELECT ?l WHERE { ex:s ex:a ?x BIND(LANG(?x) AS ?l) }",
            "l"
        ),
        vec!["\"en\""]
    );
    // DATATYPE distinguishes directional (dirLangString) from plain (langString).
    assert_eq!(
        col(
            &rete,
            "SELECT ?d WHERE { ex:s ex:a ?x BIND(DATATYPE(?x) AS ?d) }",
            "d"
        ),
        vec![format!("<{dir}>")]
    );
    assert_eq!(
        col(
            &rete,
            "SELECT ?d WHERE { ex:s ex:b ?x BIND(DATATYPE(?x) AS ?d) }",
            "d"
        ),
        vec![format!("<{plain}>")]
    );

    // A leading SPARQL 1.2 VERSION declaration parses and runs (it is dropped).
    let (_, sols) = eval_sparql(
        &rete,
        "VERSION \"1.2\" PREFIX ex: <http://ex/> SELECT ?x WHERE { ex:s ex:a ?x }",
    )
    .unwrap();
    assert_eq!(sols.len(), 1);
}

#[test]
fn property_path_zero_length_semantics() {
    // `*` and `?` include the zero-length path (a node reaches itself); `+` does
    // not. Checked in all three binding directions, since each takes a different
    // code path (forward, reversed, both-unbound enumeration).
    let rete = Rete::open(&dataset()).unwrap();

    // Forward, bound subject. Alice knows Bob and Carol directly.
    assert_eq!(
        col(&rete, "SELECT ?y WHERE { ex:Alice ex:knows? ?y }", "y"),
        vec!["<http://ex/Alice>", "<http://ex/Bob>", "<http://ex/Carol>"],
        "knows? must include Alice herself (zero-length)"
    );
    assert_eq!(
        col(&rete, "SELECT ?y WHERE { ex:Alice ex:knows* ?y }", "y"),
        vec![
            "<http://ex/Alice>",
            "<http://ex/Bob>",
            "<http://ex/Carol>",
            "<http://ex/Dave>",
            "<http://ex/Eve>",
        ],
        "knows* is the reflexive-transitive closure"
    );
    // `+` is non-reflexive: Alice only appears if a cycle returns to her (none).
    assert!(
        !col(&rete, "SELECT ?y WHERE { ex:Alice ex:knows+ ?y }", "y")
            .contains(&"<http://ex/Alice>".to_string()),
        "knows+ must NOT include Alice (no zero-length path)"
    );

    // Reversed, bound object: who reaches Carol in ≤1 hop? Carol (self), and
    // Alice/Bob who both know her directly.
    assert_eq!(
        col(&rete, "SELECT ?x WHERE { ?x ex:knows? ex:Carol }", "x"),
        vec!["<http://ex/Alice>", "<http://ex/Bob>", "<http://ex/Carol>"],
        "reversed knows? must include Carol herself"
    );

    // Both unbound: every one of the 5 people must pair with itself via `*`.
    let (_, pairs) = eval_sparql(
        &rete,
        &format!("{PREFIX}SELECT ?x ?y WHERE {{ ?x ex:knows* ?y }}"),
    )
    .unwrap();
    for who in ["Alice", "Bob", "Carol", "Dave", "Eve"] {
        let iri = format!("<http://ex/{who}>");
        assert!(
            pairs.iter().any(|b| b["x"] == iri && b["y"] == iri),
            "knows* (both unbound) must contain the self-pair for {who}"
        );
    }
}

#[test]
fn subquery_evaluates_and_joins_with_the_outer_pattern() {
    // A nested SELECT is evaluated independently; its projected solutions join
    // with the surrounding pattern on shared variables.
    let rete = Rete::open(&dataset()).unwrap();

    // A bare subquery yields the same solutions as the equivalent flat query.
    let direct = col(
        &rete,
        &format!("{PREFIX}SELECT ?p WHERE {{ ?p ex:knows ?f }}"),
        "p",
    );
    let nested = col(
        &rete,
        &format!("{PREFIX}SELECT ?p WHERE {{ {{ SELECT ?p WHERE {{ ?p ex:knows ?f }} }} }}"),
        "p",
    );
    assert_eq!(nested, direct);
    assert!(direct.contains(&"<http://ex/Alice>".to_string()));

    // The outer pattern joins on the subquery's projected variable: only people
    // Alice knows, intersected with people who know someone.
    let knowers = col(
        &rete,
        &format!(
            "{PREFIX}SELECT ?f WHERE {{ ex:Alice ex:knows ?f . \
             {{ SELECT ?f WHERE {{ ?f ex:knows ?g }} }} }}"
        ),
        "f",
    );
    // Alice knows Bob and Carol; both in turn know someone, so both survive.
    assert_eq!(
        knowers,
        vec![
            "<http://ex/Bob>".to_string(),
            "<http://ex/Carol>".to_string()
        ]
    );
}

#[test]
fn simple_and_join() {
    let rete = Rete::open(&dataset()).unwrap();
    // Who does Alice know?
    assert_eq!(
        col(&rete, "SELECT ?f WHERE { ex:Alice ex:knows ?f }", "f"),
        vec!["<http://ex/Bob>", "<http://ex/Carol>"]
    );
    // Two-hop friends of Alice (Bob->Carol, Carol->Dave).
    assert_eq!(
        col(
            &rete,
            "SELECT ?z WHERE { ex:Alice ex:knows ?y . ?y ex:knows ?z }",
            "z"
        ),
        vec!["<http://ex/Carol>", "<http://ex/Dave>"]
    );
}

#[test]
fn filter_optional_and_builtins() {
    let rete = Rete::open(&dataset()).unwrap();
    // People older than 32.
    assert_eq!(
        col(
            &rete,
            "SELECT ?p WHERE { ?p ex:age ?a . FILTER(?a > 32) }",
            "p"
        ),
        vec!["<http://ex/Carol>", "<http://ex/Dave>"]
    );
    // Eve has no age; OPTIONAL keeps her but ?a is unbound, so a name filter
    // still returns all five.
    let names = col(
        &rete,
        "SELECT ?p WHERE { ?p ex:name ?n . OPTIONAL { ?p ex:age ?a } }",
        "p",
    );
    assert_eq!(names.len(), 5);
}

#[test]
fn aggregate_path_order_union_minus() {
    let rete = Rete::open(&dataset()).unwrap();

    // GROUP BY COUNT: out-degree per person.
    let (_, deg) = eval_sparql(
        &rete,
        &format!("{PREFIX}SELECT ?p (COUNT(?f) AS ?n) WHERE {{ ?p ex:knows ?f }} GROUP BY ?p"),
    )
    .unwrap();
    let alice = deg.iter().find(|b| b["p"] == "<http://ex/Alice>").unwrap();
    assert_eq!(
        alice["n"],
        "\"2\"^^<http://www.w3.org/2001/XMLSchema#integer>"
    );

    // Transitive reach from Alice (everyone downstream).
    assert_eq!(
        col(&rete, "SELECT ?y WHERE { ex:Alice ex:knows+ ?y }", "y"),
        vec![
            "<http://ex/Bob>",
            "<http://ex/Carol>",
            "<http://ex/Dave>",
            "<http://ex/Eve>",
        ]
    );

    // ORDER BY age DESC, take the two oldest.
    let (_, oldest) = eval_sparql(
        &rete,
        &format!("{PREFIX}SELECT ?p WHERE {{ ?p ex:age ?a }} ORDER BY DESC(?a) LIMIT 2"),
    )
    .unwrap();
    assert_eq!(oldest[0]["p"], "<http://ex/Dave>");
    assert_eq!(oldest[1]["p"], "<http://ex/Carol>");

    // MINUS: people Alice knows who themselves know nobody → none (all of
    // Alice's friends know someone).
    assert!(col(
        &rete,
        "SELECT ?f WHERE { ex:Alice ex:knows ?f . MINUS { ?f ex:knows ?x } }",
        "f"
    )
    .is_empty());
}

/// A 3-graph dataset built through the public dataset API.
fn dataset_3graph() -> Vec<u8> {
    let triples = [
        // default graph
        ("Alice", "type", "Person", None),
        ("Bob", "type", "Person", None),
        // friends graph
        ("Alice", "knows", "Bob", Some("g/friends")),
        ("Bob", "knows", "Carol", Some("g/friends")),
        // facts graph
        ("Carol", "city", "NYC", Some("g/facts")),
        ("Bob", "city", "NYC", Some("g/facts")),
    ];
    let mut db = DictionaryBuilder::new();
    for (s, p, o, _) in triples {
        db.observe(&iri(s), &iri(p), &term(o));
    }
    let dict = db.build();

    use std::collections::BTreeMap;
    let mut def = GraphIndexBuilder::new();
    let mut named: BTreeMap<String, GraphIndexBuilder> = BTreeMap::new();
    for (s, p, o, g) in triples {
        let t = dict.encode(&iri(s), &iri(p), &term(o)).unwrap();
        match g {
            None => def.push(t),
            Some(name) => named.entry(iri(name)).or_default().push(t),
        }
    }
    let named_idx: Vec<(String, _)> = named.into_iter().map(|(g, b)| (g, b.build())).collect();
    write_dataset(&dict, &def.build(), &named_idx, true, &[], 0)
}

#[test]
fn dataset_graph_from_describe() {
    let rete = Rete::open(&dataset_3graph()).unwrap();
    let p = "PREFIX ex: <http://ex/> ";

    // GRAPH <iri>: knows edges live only in the friends graph. (Graph IRIs
    // contain '/', so they must be written in full, not as prefixed names.)
    let (_, s) = eval_sparql(
        &rete,
        &format!("{p}SELECT ?f WHERE {{ GRAPH <http://ex/g/friends> {{ ex:Alice ex:knows ?f }} }}"),
    )
    .unwrap();
    assert_eq!(s.len(), 1);
    assert_eq!(s[0]["f"], "<http://ex/Bob>");

    // GRAPH ?g: which graphs mention NYC?
    let (_, g) = eval_sparql(
        &rete,
        &format!("{p}SELECT ?g WHERE {{ GRAPH ?g {{ ?x ex:city <http://ex/NYC> }} }}"),
    )
    .unwrap();
    assert!(g.iter().all(|b| b["g"] == "<http://ex/g/facts>"));

    // FROM merges friends+facts so a cross-graph join works:
    // Bob (knows, in friends) lives in NYC (city, in facts).
    let (_, j) = eval_sparql(
        &rete,
        &format!(
            "{p}SELECT ?f FROM <http://ex/g/friends> FROM <http://ex/g/facts> \
             WHERE {{ ex:Alice ex:knows ?f . ?f ex:city <http://ex/NYC> }}"
        ),
    )
    .unwrap();
    assert_eq!(j.len(), 1);
    assert_eq!(j[0]["f"], "<http://ex/Bob>");

    // DESCRIBE works against the default graph.
    match eval_query(&rete, "DESCRIBE <http://ex/Alice>").unwrap() {
        QueryOutput::Construct(t) => assert_eq!(t.len(), 1), // Alice type Person
        other => panic!("describe: {other:?}"),
    }
}

#[test]
fn ask_construct_exists() {
    let rete = Rete::open(&dataset()).unwrap();

    match eval_query(&rete, &format!("{PREFIX}ASK {{ ?a ex:knows ?b }}")).unwrap() {
        QueryOutput::Ask(b) => assert!(b),
        _ => panic!("ask"),
    }

    // CONSTRUCT a reverse graph; Alice should be known-by nobody here? She is
    // known-by Carol (Carol knows ... no). Check Bob is knownBy Alice.
    match eval_query(
        &rete,
        &format!("{PREFIX}CONSTRUCT {{ ?b ex:knownBy ?a }} WHERE {{ ?a ex:knows ?b }}"),
    )
    .unwrap()
    {
        QueryOutput::Construct(triples) => assert!(triples.contains(&(
            "<http://ex/Bob>".into(),
            "<http://ex/knownBy>".into(),
            "<http://ex/Alice>".into(),
        ))),
        _ => panic!("construct"),
    }

    // FILTER EXISTS: people who know someone in NYC.
    let knows_nyc = col(
        &rete,
        "SELECT ?p WHERE { ?p ex:knows ?f . FILTER EXISTS { ?f ex:city <http://ex/city/NYC> } }",
        "p",
    );
    // Alice->Carol(NYC), Bob->Carol(NYC).
    assert_eq!(knows_nyc, vec!["<http://ex/Alice>", "<http://ex/Bob>"]);
}

/// A dependency graph (SBOM-style): a vulnerable leaf and `dependsOn` chains.
/// Mirrors `examples/deps.nt` — the offline/embedded "what does this CVE impact?"
/// use case, answered by a transitive property path entirely in-process.
fn deps() -> Vec<u8> {
    let rt = "<http://www.w3.org/1999/02/22-rdf-syntax-ns#type>";
    let t: Vec<(String, String, String)> = vec![
        ("app", rt, "Application"),
        ("web", rt, "Library"),
        ("auth", rt, "Library"),
        ("logging", rt, "Library"),
        ("log4x", rt, "Library"),
        ("safejson", rt, "Library"),
        ("log4x", "hasVulnerability", "CVE-2099-0001"),
        ("app", "dependsOn", "web"),
        ("app", "dependsOn", "auth"),
        ("web", "dependsOn", "logging"),
        ("auth", "dependsOn", "logging"),
        ("auth", "dependsOn", "safejson"),
        ("logging", "dependsOn", "log4x"),
    ]
    .into_iter()
    .map(|(s, p, o)| {
        let pred = if p == rt { rt.to_string() } else { iri(p) };
        (iri(s), pred, iri(o))
    })
    .collect();
    let refs: Vec<(&str, &str, &str)> = t
        .iter()
        .map(|(s, p, o)| (s.as_str(), p.as_str(), o.as_str()))
        .collect();
    build(&refs)
}

#[test]
fn transitive_dependency_impact() {
    let rete = Rete::open(&deps()).unwrap();
    // Everything that (transitively) depends on the vulnerable package: the
    // reverse-reachability query a frontend would run on click of a CVE.
    let impacted = col(
        &rete,
        "SELECT DISTINCT ?dependent WHERE { ?dependent ex:dependsOn+ ex:log4x }",
        "dependent",
    );
    // app -> web/auth -> logging -> log4x; safejson is off the vulnerable path.
    assert_eq!(
        impacted,
        vec![
            "<http://ex/app>",
            "<http://ex/auth>",
            "<http://ex/logging>",
            "<http://ex/web>",
        ]
    );

    // The same query, joined to the CVE id and restricted to Libraries — the
    // shape a real impact report would use (type filter + the vuln's identifier).
    let rt = "<http://www.w3.org/1999/02/22-rdf-syntax-ns#type>";
    let report = col(
        &rete,
        &format!(
            "SELECT DISTINCT ?lib WHERE {{ \
               ?lib ex:dependsOn+ ?v . ?v ex:hasVulnerability ?cve . \
               ?lib {rt} ex:Library }}"
        ),
        "lib",
    );
    // Libraries on the vulnerable path: web, auth, logging (not app — it's an
    // Application; not safejson — it doesn't reach the vuln).
    assert_eq!(
        report,
        vec!["<http://ex/auth>", "<http://ex/logging>", "<http://ex/web>"]
    );
}

#[test]
fn datatype_and_lang_builtins() {
    // A graph with the three recovered-from-Wikidata literal kinds: a typed
    // numeric, a typed dateTime, a language-tagged string, and a plain string.
    let dt = "<http://www.w3.org/2001/XMLSchema#dateTime>";
    let t: Vec<(String, String, String)> = vec![
        (
            "Q1",
            "pop",
            "\"42\"^^<http://www.w3.org/2001/XMLSchema#integer>",
        ),
        ("Q1", "born", &format!("\"2001-05-11T00:00:00Z\"^^{dt}")),
        ("Q1", "label", "\"Douglas\"@en"),
        ("Q1", "code", "\"plain\""),
    ]
    .into_iter()
    .map(|(s, p, o)| (iri(s), iri(p), term(o)))
    .collect();
    let refs: Vec<(&str, &str, &str)> = t
        .iter()
        .map(|(s, p, o)| (s.as_str(), p.as_str(), o.as_str()))
        .collect();
    let rete = Rete::open(&build(&refs)).unwrap();

    // DATATYPE filter selects only the dateTime-typed object.
    assert_eq!(
        col(
            &rete,
            &format!("SELECT ?p WHERE {{ ex:Q1 ?p ?o FILTER(DATATYPE(?o) = {dt}) }}"),
            "p"
        ),
        vec!["<http://ex/born>"]
    );
    // A plain literal is xsd:string; a language-tagged one is rdf:langString.
    assert_eq!(
        col(
            &rete,
            "SELECT ?p WHERE { ex:Q1 ?p ?o \
             FILTER(DATATYPE(?o) = <http://www.w3.org/2001/XMLSchema#string>) }",
            "p"
        ),
        vec!["<http://ex/code>"]
    );
    assert_eq!(
        col(
            &rete,
            "SELECT ?p WHERE { ex:Q1 ?p ?o \
             FILTER(DATATYPE(?o) = <http://www.w3.org/1999/02/22-rdf-syntax-ns#langString>) }",
            "p"
        ),
        vec!["<http://ex/label>"]
    );
    // LANG selects the language-tagged literal; plain/typed literals have "".
    assert_eq!(
        col(
            &rete,
            "SELECT ?p WHERE { ex:Q1 ?p ?o FILTER(LANG(?o) = \"en\") }",
            "p"
        ),
        vec!["<http://ex/label>"]
    );
}
