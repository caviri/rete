use rete_core::{validate_shacl, DataGraph, ShaclShapes};

fn graph(triples: &[(&str, &str, &str)]) -> DataGraph {
    DataGraph::from_triples(
        triples
            .iter()
            .map(|(s, p, o)| (s.to_string(), p.to_string(), o.to_string()))
            .collect(),
    )
}

fn has_component(report: &rete_core::ValidationReport, component: &str) -> bool {
    report
        .results
        .iter()
        .any(|r| r.source_constraint_component == component)
}

#[test]
fn validates_target_class_property_constraints_and_reports_messages() {
    const TYPE: &str = "<http://www.w3.org/1999/02/22-rdf-syntax-ns#type>";
    const SUBCLASS: &str = "<http://www.w3.org/2000/01/rdf-schema#subClassOf>";
    const PERSON: &str = "<http://ex/Person>";
    const EMPLOYEE: &str = "<http://ex/Employee>";
    const ALICE: &str = "<http://ex/alice>";
    const BOB: &str = "<http://ex/bob>";
    const EMAIL: &str = "<http://ex/email>";
    const AGE: &str = "<http://ex/age>";

    let data = graph(&[
        (EMPLOYEE, SUBCLASS, PERSON),
        (ALICE, TYPE, PERSON),
        (ALICE, EMAIL, "\"alice@example.org\""),
        (
            ALICE,
            AGE,
            "\"34\"^^<http://www.w3.org/2001/XMLSchema#integer>",
        ),
        (BOB, TYPE, EMPLOYEE),
        (BOB, EMAIL, "\"bad\""),
        (BOB, EMAIL, "\"other@example.org\""),
        (BOB, AGE, "\"seventeen\""),
    ]);
    let shapes = ShaclShapes::parse_turtle(
        r#"
        @prefix ex: <http://ex/> .
        @prefix sh: <http://www.w3.org/ns/shacl#> .
        @prefix xsd: <http://www.w3.org/2001/XMLSchema#> .

        ex:PersonShape a sh:NodeShape ;
          sh:targetClass ex:Person ;
          sh:nodeKind sh:IRI ;
          sh:property [
            sh:path ex:email ;
            sh:minCount 1 ;
            sh:maxCount 1 ;
            sh:datatype xsd:string ;
            sh:pattern "^[^@]+@[^@]+$" ;
            sh:message "People need exactly one valid email"
          ] ;
          sh:property [
            sh:path ex:age ;
            sh:datatype xsd:integer ;
            sh:minInclusive 0
          ] .
        "#,
    )
    .unwrap();

    let report = validate_shacl(&data, &shapes);

    assert!(!report.conforms);
    assert!(has_component(
        &report,
        "http://www.w3.org/ns/shacl#MaxCountConstraintComponent"
    ));
    assert!(has_component(
        &report,
        "http://www.w3.org/ns/shacl#PatternConstraintComponent"
    ));
    assert!(has_component(
        &report,
        "http://www.w3.org/ns/shacl#DatatypeConstraintComponent"
    ));
    assert!(report.results.iter().any(|r| r.focus_node == BOB
        && r.messages
            .iter()
            .any(|m| m == "People need exactly one valid email")));
    assert!(!report.results.iter().any(|r| r.focus_node == ALICE));
}

#[test]
fn supports_paths_nested_shapes_closed_shapes_and_report_serialization() {
    const ALICE: &str = "<http://ex/alice>";
    const PARENT: &str = "<http://ex/parent>";
    const EXTRA: &str = "<http://ex/extra>";

    let data = graph(&[
        (ALICE, PARENT, "<http://ex/bob>"),
        ("<http://ex/bob>", PARENT, "<http://ex/carol>"),
        (
            "<http://ex/carol>",
            "<http://ex/email>",
            "\"carol@example.org\"",
        ),
        (ALICE, EXTRA, "\"surprise\""),
    ]);
    let shapes = ShaclShapes::parse_turtle(
        r#"
        @prefix ex: <http://ex/> .
        @prefix sh: <http://www.w3.org/ns/shacl#> .
        @prefix rdf: <http://www.w3.org/1999/02/22-rdf-syntax-ns#> .

        ex:AliceShape a sh:NodeShape ;
          sh:targetNode ex:alice ;
          sh:closed true ;
          sh:ignoredProperties ( rdf:type ) ;
          sh:property [
            sh:path ex:parent ;
            sh:minCount 1 ;
            sh:node ex:IriNode
          ] ;
          sh:property [
            sh:path ( ex:parent ex:parent ex:email ) ;
            sh:hasValue "carol@example.org"
          ] .

        ex:IriNode a sh:NodeShape ;
          sh:nodeKind sh:IRI .
        "#,
    )
    .unwrap();

    let report = validate_shacl(&data, &shapes);

    assert!(!report.conforms);
    assert!(has_component(
        &report,
        "http://www.w3.org/ns/shacl#ClosedConstraintComponent"
    ));
    assert!(!has_component(
        &report,
        "http://www.w3.org/ns/shacl#NodeConstraintComponent"
    ));
    assert!(!has_component(
        &report,
        "http://www.w3.org/ns/shacl#HasValueConstraintComponent"
    ));
    assert!(report.to_json().contains("\"conforms\": false"));
    assert!(report
        .to_turtle()
        .contains("<http://www.w3.org/ns/shacl#ValidationReport>"));
}

#[test]
fn supports_inverse_paths_enumerations_language_and_qualified_counts() {
    const KNOWS: &str = "<http://ex/knows>";
    const LABEL: &str = "<http://ex/label>";
    const STATUS: &str = "<http://ex/status>";

    let data = graph(&[
        ("<http://ex/alice>", KNOWS, "<http://ex/bob>"),
        ("<http://ex/carla>", KNOWS, "<http://ex/bob>"),
        ("<http://ex/bob>", LABEL, "\"Bob\"@en"),
        ("<http://ex/bob>", LABEL, "\"Bobby\"@en"),
        ("<http://ex/bob>", STATUS, "<http://ex/Archived>"),
        ("<http://ex/bob>", "<http://ex/tag>", "\"primary\""),
        ("<http://ex/bob>", "<http://ex/tag>", "\"secondary\""),
    ]);
    let shapes = ShaclShapes::parse_turtle(
        r#"
        @prefix ex: <http://ex/> .
        @prefix sh: <http://www.w3.org/ns/shacl#> .
        @prefix xsd: <http://www.w3.org/2001/XMLSchema#> .

        ex:KnownPersonShape a sh:NodeShape ;
          sh:targetObjectsOf ex:knows ;
          sh:property [
            sh:path [ sh:inversePath ex:knows ] ;
            sh:minCount 2
          ] ;
          sh:property [
            sh:path ex:label ;
            sh:languageIn ( "en" "fr" ) ;
            sh:uniqueLang true
          ] ;
          sh:property [
            sh:path ex:status ;
            sh:in ( ex:Active ex:Inactive )
          ] ;
          sh:property [
            sh:path ex:tag ;
            sh:qualifiedValueShape [ sh:datatype xsd:string ] ;
            sh:qualifiedMinCount 3
          ] .
        "#,
    )
    .unwrap();

    let report = validate_shacl(&data, &shapes);

    assert!(!report.conforms);
    assert!(has_component(
        &report,
        "http://www.w3.org/ns/shacl#UniqueLangConstraintComponent"
    ));
    assert!(has_component(
        &report,
        "http://www.w3.org/ns/shacl#InConstraintComponent"
    ));
    assert!(has_component(
        &report,
        "http://www.w3.org/ns/shacl#QualifiedMinCountConstraintComponent"
    ));
    assert!(!has_component(
        &report,
        "http://www.w3.org/ns/shacl#MinCountConstraintComponent"
    ));
}

#[test]
fn qualified_value_shapes_disjoint_excludes_overlapping_sibling_matches_from_counts() {
    const TYPE: &str = "<http://www.w3.org/1999/02/22-rdf-syntax-ns#type>";

    let data = graph(&[
        ("<http://ex/hand>", TYPE, "<http://ex/Hand>"),
        ("<http://ex/hand>", "<http://ex/digit>", "<http://ex/thumb>"),
        ("<http://ex/hand>", "<http://ex/digit>", "<http://ex/index>"),
        (
            "<http://ex/hand>",
            "<http://ex/digit>",
            "<http://ex/middle>",
        ),
        ("<http://ex/hand>", "<http://ex/digit>", "<http://ex/ring>"),
        ("<http://ex/hand>", "<http://ex/digit>", "<http://ex/pinky>"),
        ("<http://ex/thumb>", TYPE, "<http://ex/Thumb>"),
        ("<http://ex/thumb>", TYPE, "<http://ex/Finger>"),
        ("<http://ex/index>", TYPE, "<http://ex/Finger>"),
        ("<http://ex/middle>", TYPE, "<http://ex/Finger>"),
        ("<http://ex/ring>", TYPE, "<http://ex/Finger>"),
        ("<http://ex/pinky>", TYPE, "<http://ex/Finger>"),
    ]);
    let shapes = ShaclShapes::parse_turtle(
        r#"
        @prefix ex: <http://ex/> .
        @prefix sh: <http://www.w3.org/ns/shacl#> .

        ex:HandShape a sh:NodeShape ;
          sh:targetClass ex:Hand ;
          sh:property [
            sh:path ex:digit ;
            sh:qualifiedValueShape [ sh:class ex:Thumb ] ;
            sh:qualifiedValueShapesDisjoint true ;
            sh:qualifiedMinCount 1 ;
            sh:qualifiedMaxCount 1
          ] ;
          sh:property [
            sh:path ex:digit ;
            sh:qualifiedValueShape [ sh:class ex:Finger ] ;
            sh:qualifiedValueShapesDisjoint true ;
            sh:qualifiedMinCount 4 ;
            sh:qualifiedMaxCount 4
          ] .
        "#,
    )
    .unwrap();

    let report = validate_shacl(&data, &shapes);

    assert!(!report.conforms);
    assert!(
        has_component(
            &report,
            "http://www.w3.org/ns/shacl#QualifiedMinCountConstraintComponent"
        ),
        "the overlapping thumb is excluded from the Thumb count because it also matches the sibling Finger shape"
    );
    assert!(
        !has_component(
            &report,
            "http://www.w3.org/ns/shacl#QualifiedMaxCountConstraintComponent"
        ),
        "the overlapping thumb is also excluded from the Finger count, leaving exactly four fingers"
    );
}
