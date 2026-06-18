//! The standardized exploration query library (Design B of `dev/pyramid-dev.md`).
//!
//! A vetted set of starter SPARQL queries, **auto-instantiated at build** with a
//! dataset's real vocabulary (`{{TOP_CLASS}}`/`{{TOP_PRED}}`/`{{LABEL_PRED}}`/…)
//! drawn from the enriched [`DatasetCard`] profile, and emitted **only when the
//! required signal is present** (no geo query without geometry, no time query
//! without a time predicate) so the shipped set always returns rows. Every query
//! carries its own `PREFIX` block (the engine injects none) and is tagged with
//! the cheapest [`Tier`] that can answer it.
//!
//! This is pure CLI/serde generation — no format change.

use super::card::{DatasetCard, ExampleQuery, Tier};
use rete_core::RDF_TYPE;

/// Prepended to every generated query — the engine parses with **zero** implicit
/// prefixes (`Query::parse(q, None)`), so an undeclared prefix is a parse error.
const PREFIXES: &str = "\
PREFIX rdf:  <http://www.w3.org/1999/02/22-rdf-syntax-ns#>
PREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>
PREFIX owl:  <http://www.w3.org/2002/07/owl#>
PREFIX void: <http://rdfs.org/ns/void#>
PREFIX dct:  <http://purl.org/dc/terms/>
PREFIX skos: <http://www.w3.org/2004/02/skos/core#>
PREFIX geo:  <http://www.opengis.net/ont/geosparql#>
PREFIX geof: <http://www.opengis.net/def/function/geosparql/>
PREFIX wgs:  <http://www.w3.org/2003/01/geo/wgs84_pos#>
PREFIX time: <http://www.w3.org/2006/time#>
PREFIX xsd:  <http://www.w3.org/2001/XMLSchema#>";

/// A capability the dataset must offer for a template to be emitted (and, for the
/// string-valued ones, the vocabulary substituted into its placeholder).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Cap {
    TopClass,
    TopPred,
    LabelPred,
    TimePred,
    NumPred,
    BaseIri,
    HubIri,
    GeoLatLong,
    GeoWkt,
    NamedGraphs,
    Link,
    HasLiterals,
}

impl Cap {
    /// Stable key recorded in `ExampleQuery.requires`.
    fn key(self) -> &'static str {
        match self {
            Cap::TopClass => "TOP_CLASS",
            Cap::TopPred => "TOP_PRED",
            Cap::LabelPred => "LABEL_PRED",
            Cap::TimePred => "TIME_PRED",
            Cap::NumPred => "NUM_PRED",
            Cap::BaseIri => "BASE_IRI",
            Cap::HubIri => "HUB_IRI",
            Cap::GeoLatLong => "GEO_LATLONG",
            Cap::GeoWkt => "GEO_WKT",
            Cap::NamedGraphs => "NAMED_GRAPHS",
            Cap::Link => "LINK",
            Cap::HasLiterals => "HAS_LITERALS",
        }
    }
}

/// One query template: a body with `{{PLACEHOLDER}}`s, the tier that answers it,
/// and the capabilities it needs (which also gate emission).
struct Template {
    id: &'static str,
    title: &'static str,
    dimension: &'static str,
    question: &'static str,
    body: &'static str,
    tier: Tier,
    requires: &'static [Cap],
}

/// The library. Order is fixed → deterministic output (folded into the hash).
const TEMPLATES: &[Template] = &[
    // --- Overview ---
    Template {
        id: "ov-triples",
        title: "How many statements?",
        dimension: "overview",
        question: "How big is this graph — how many triples?",
        body: "SELECT (COUNT(*) AS ?n) WHERE { ?s ?p ?o }",
        tier: Tier::Summary,
        requires: &[],
    },
    Template {
        id: "ov-pred-list",
        title: "What relationships exist?",
        dimension: "overview",
        question: "Which predicates (relationships) appear in the data?",
        body: "SELECT DISTINCT ?p WHERE { ?s ?p ?o }",
        tier: Tier::Summary,
        requires: &[],
    },
    Template {
        id: "ov-pred-hist",
        title: "How frequent is each relationship?",
        dimension: "overview",
        question: "How many statements use each predicate?",
        // No ORDER BY — keeps it on the index-free summary fast path; sort client-side.
        body: "SELECT ?p (COUNT(*) AS ?n) WHERE { ?s ?p ?o } GROUP BY ?p",
        tier: Tier::Summary,
        requires: &[],
    },
    Template {
        id: "ov-ask-pred",
        title: "Does the top predicate occur?",
        dimension: "overview",
        question: "Is the most common predicate present at all?",
        body: "ASK { ?s {{TOP_PRED}} ?o }",
        tier: Tier::Summary,
        requires: &[Cap::TopPred],
    },
    // --- Identity & labels ---
    Template {
        id: "id-sample",
        title: "A real entity to start from",
        dimension: "identity",
        question: "Give me one concrete entity of the most common type.",
        body: "SELECT ?s WHERE { ?s a {{TOP_CLASS}} } LIMIT 1",
        tier: Tier::Index,
        requires: &[Cap::TopClass],
    },
    Template {
        id: "lb-labels",
        title: "Readable names for entities",
        dimension: "labels",
        question: "What are the human-readable names of some entities?",
        body: "SELECT ?s ?label WHERE { ?s a {{TOP_CLASS}} ; {{LABEL_PRED}} ?label } LIMIT 50",
        tier: Tier::Index,
        requires: &[Cap::TopClass, Cap::LabelPred],
    },
    // --- Types ---
    Template {
        id: "ty-class-hist",
        title: "What kinds of things, how many?",
        dimension: "types",
        question: "How many entities of each class are there?",
        body: "SELECT ?c (COUNT(?s) AS ?n) WHERE { ?s a ?c } GROUP BY ?c ORDER BY DESC(?n) LIMIT 50",
        tier: Tier::Index,
        requires: &[Cap::TopClass],
    },
    Template {
        id: "ty-entities",
        title: "How many distinct typed entities?",
        dimension: "types",
        question: "How many distinct entities carry a type?",
        body: "SELECT (COUNT(DISTINCT ?s) AS ?n) WHERE { ?s a [] }",
        tier: Tier::Index,
        requires: &[Cap::TopClass],
    },
    Template {
        id: "ty-class-shape",
        title: "What does the top class look like?",
        dimension: "types",
        question: "Which predicates describe the most common class?",
        body: "SELECT ?p (COUNT(*) AS ?n) WHERE { ?s a {{TOP_CLASS}} ; ?p ?o } GROUP BY ?p ORDER BY DESC(?n)",
        tier: Tier::Index,
        requires: &[Cap::TopClass],
    },
    // --- Topology ---
    Template {
        id: "top-class-graph",
        title: "Which classes link to which?",
        dimension: "topology",
        question: "What is the class-to-class schema graph?",
        body: "SELECT ?sC ?p ?oC (COUNT(*) AS ?n) WHERE { ?s a ?sC ; ?p ?o OPTIONAL { ?o a ?oC } } GROUP BY ?sC ?p ?oC ORDER BY DESC(?n)",
        tier: Tier::Index,
        requires: &[Cap::TopClass],
    },
    Template {
        id: "top-out-hubs",
        title: "Most connected sources",
        dimension: "topology",
        question: "Which subjects make the most statements?",
        body: "SELECT ?s (COUNT(*) AS ?d) WHERE { ?s ?p ?o } GROUP BY ?s ORDER BY DESC(?d) LIMIT 25",
        tier: Tier::Index,
        requires: &[],
    },
    Template {
        id: "top-in-hubs",
        title: "Most referenced entities",
        dimension: "topology",
        question: "Which resources are referenced most often?",
        body: "SELECT ?o (COUNT(*) AS ?d) WHERE { ?s ?p ?o FILTER(!isLiteral(?o)) } GROUP BY ?o ORDER BY DESC(?d) LIMIT 25",
        tier: Tier::Index,
        requires: &[],
    },
    Template {
        id: "top-dangling",
        title: "Referenced but undescribed IRIs",
        dimension: "topology",
        question: "Which IRIs are referenced as objects but never described?",
        body: "SELECT ?o WHERE { ?s ?p ?o FILTER(isIRI(?o)) FILTER NOT EXISTS { ?o ?p2 ?o2 } } LIMIT 100",
        tier: Tier::Index,
        requires: &[],
    },
    Template {
        id: "top-reach",
        title: "What's reachable from a hub?",
        dimension: "connectivity",
        question: "What can you reach from the busiest hub via the top predicate?",
        body: "SELECT ?y WHERE { {{HUB_IRI}} {{TOP_PRED}}+ ?y } LIMIT 100",
        tier: Tier::Index,
        requires: &[Cap::HubIri, Cap::TopPred],
    },
    // --- Connectivity / completeness ---
    Template {
        id: "cmp-coverage",
        title: "Do all top-class entities have a label?",
        dimension: "connectivity",
        question: "How complete is labelling on the most common class?",
        body: "SELECT (COUNT(?s) AS ?total) (COUNT(?l) AS ?have) WHERE { ?s a {{TOP_CLASS}} OPTIONAL { ?s {{LABEL_PRED}} ?l } }",
        tier: Tier::Index,
        requires: &[Cap::TopClass, Cap::LabelPred],
    },
    // --- Links ---
    Template {
        id: "lk-sameas",
        title: "Aligned to which external datasets?",
        dimension: "links",
        question: "Which entity-alignment predicates are used, and how often?",
        body: "SELECT ?p (COUNT(*) AS ?n) WHERE { ?s ?p ?o VALUES ?p { owl:sameAs skos:exactMatch rdfs:seeAlso } } GROUP BY ?p",
        tier: Tier::Index,
        requires: &[Cap::Link],
    },
    Template {
        id: "lk-external",
        title: "Which predicates link out?",
        dimension: "links",
        question: "Which predicates point to IRIs outside this dataset?",
        body: "SELECT ?p (COUNT(*) AS ?n) WHERE { ?s ?p ?o FILTER(isIRI(?o) && !STRSTARTS(STR(?o), \"{{BASE_IRI}}\")) } GROUP BY ?p ORDER BY DESC(?n) LIMIT 50",
        tier: Tier::Index,
        requires: &[Cap::BaseIri],
    },
    // --- Literals ---
    Template {
        id: "lt-datatypes",
        title: "What value types appear?",
        dimension: "literals",
        question: "Which literal datatypes are used, and how often?",
        body: "SELECT ?dt (COUNT(*) AS ?n) WHERE { ?s ?p ?o FILTER(isLiteral(?o)) } GROUP BY (DATATYPE(?o) AS ?dt) ORDER BY DESC(?n)",
        tier: Tier::Index,
        requires: &[Cap::HasLiterals],
    },
    Template {
        id: "lt-langs",
        title: "What languages?",
        dimension: "literals",
        question: "Which language tags appear on literals, and how often?",
        body: "SELECT ?l (COUNT(*) AS ?n) WHERE { ?s ?p ?o FILTER(isLiteral(?o)) } GROUP BY (LANG(?o) AS ?l) ORDER BY DESC(?n)",
        tier: Tier::Index,
        requires: &[Cap::HasLiterals],
    },
    Template {
        id: "lt-numrange",
        title: "Range of a numeric property",
        dimension: "literals",
        question: "What is the min / average / max of the top numeric property?",
        // BIND the cast first — aggregating over a function expression is rejected.
        body: "SELECT (MIN(?v) AS ?lo) (AVG(?v) AS ?avg) (MAX(?v) AS ?hi) WHERE { ?s {{NUM_PRED}} ?o BIND(xsd:double(?o) AS ?v) }",
        tier: Tier::Index,
        requires: &[Cap::NumPred],
    },
    // --- Time ---
    Template {
        id: "ti-extent",
        title: "What period does it span?",
        dimension: "time",
        question: "What is the earliest and latest value of the top time predicate?",
        body: "SELECT (MIN(?d) AS ?from) (MAX(?d) AS ?to) WHERE { ?s {{TIME_PRED}} ?d }",
        tier: Tier::Index,
        requires: &[Cap::TimePred],
    },
    Template {
        id: "ti-histogram",
        title: "Distribution over time",
        dimension: "time",
        question: "How are entities distributed by year?",
        body: "SELECT ?yr (COUNT(*) AS ?n) WHERE { ?s {{TIME_PRED}} ?d BIND(SUBSTR(STR(?d),1,4) AS ?yr) } GROUP BY ?yr ORDER BY ?yr",
        tier: Tier::Index,
        requires: &[Cap::TimePred],
    },
    // --- Space (WKT is always CRS84 lon/lat) ---
    Template {
        id: "sp-bbox",
        title: "Geographic extent",
        dimension: "space",
        question: "What is the bounding box of the wgs84 coordinates? (lon/lat)",
        body: "SELECT (MIN(?lon) AS ?minLon) (MIN(?lat) AS ?minLat) (MAX(?lon) AS ?maxLon) (MAX(?lat) AS ?maxLat) WHERE { ?s wgs:long ?lon ; wgs:lat ?lat }",
        tier: Tier::Index,
        requires: &[Cap::GeoLatLong],
    },
    Template {
        id: "sp-within",
        title: "Features inside a box",
        dimension: "space",
        question: "Which features fall inside the dataset's bounding box? (WKT is lon/lat)",
        body: "SELECT ?s WHERE { ?s geo:hasGeometry/geo:asWKT ?w FILTER(geof:sfWithin(?w, \"{{BBOX_POLYGON}}\"^^geo:wktLiteral)) } LIMIT 100",
        tier: Tier::Index,
        requires: &[Cap::GeoWkt],
    },
    // --- Named graphs ---
    Template {
        id: "ng-list",
        title: "What graphs partition this?",
        dimension: "graphs",
        question: "Which named graphs does the dataset contain?",
        body: "SELECT DISTINCT ?g WHERE { GRAPH ?g { ?s ?p ?o } }",
        tier: Tier::Index,
        requires: &[Cap::NamedGraphs],
    },
    Template {
        id: "ng-sizes",
        title: "How big is each graph?",
        dimension: "graphs",
        question: "How many triples are in each named graph?",
        body: "SELECT ?g (COUNT(*) AS ?n) WHERE { GRAPH ?g { ?s ?p ?o } } GROUP BY ?g ORDER BY DESC(?n)",
        tier: Tier::Index,
        requires: &[Cap::NamedGraphs],
    },
];

/// The dataset's resolved vocabulary, drawn from the card profile.
struct Caps {
    top_class: Option<String>,
    top_pred: Option<String>,
    label_pred: Option<String>,
    time_pred: Option<String>,
    num_pred: Option<String>,
    base_iri: Option<String>,
    hub_iri: Option<String>,
    bbox_polygon: Option<String>,
    geo_latlong: bool,
    geo_wkt: bool,
    named_graphs: bool,
    link: bool,
    has_literals: bool,
}

impl Caps {
    fn from_card(card: &DatasetCard) -> Self {
        let s = &card.signals;
        // The most frequent predicate that is not rdf:type (the interesting
        // relation), falling back to whatever is most frequent.
        let top_pred = card
            .predicates
            .iter()
            .map(|(p, _)| p)
            .find(|p| p.as_str() != RDF_TYPE)
            .or_else(|| card.predicates.first().map(|(p, _)| p))
            .cloned();
        let bbox_polygon = if s.geo_wkt {
            Some(bbox_polygon(s.spatial_bbox))
        } else {
            None
        };
        Caps {
            top_class: card.classes.first().map(|(c, _)| c.clone()),
            top_pred,
            label_pred: s.label_predicate.clone(),
            time_pred: s.time_predicates.first().cloned(),
            num_pred: s.numeric_predicates.first().cloned(),
            base_iri: s.base_iri.clone(),
            hub_iri: card.top_hubs.first().map(|(h, _)| h.clone()),
            bbox_polygon,
            geo_latlong: s.geo_latlong,
            geo_wkt: s.geo_wkt,
            named_graphs: card.named_graph_count > 0,
            link: !s.link_predicates.is_empty(),
            has_literals: !card.datatypes.is_empty(),
        }
    }

    /// Is a capability available (string caps must resolve; bool caps must hold)?
    fn available(&self, cap: Cap) -> bool {
        match cap {
            Cap::TopClass => self.top_class.is_some(),
            Cap::TopPred => self.top_pred.is_some(),
            Cap::LabelPred => self.label_pred.is_some(),
            Cap::TimePred => self.time_pred.is_some(),
            Cap::NumPred => self.num_pred.is_some(),
            Cap::BaseIri => self.base_iri.is_some(),
            Cap::HubIri => self.hub_iri.is_some(),
            Cap::GeoLatLong => self.geo_latlong,
            Cap::GeoWkt => self.geo_wkt,
            Cap::NamedGraphs => self.named_graphs,
            Cap::Link => self.link,
            Cap::HasLiterals => self.has_literals,
        }
    }

    /// Substitute every placeholder we have a value for.
    fn substitute(&self, body: &str) -> String {
        let mut out = body.to_string();
        let pairs: [(&str, &Option<String>); 8] = [
            ("{{TOP_CLASS}}", &self.top_class),
            ("{{TOP_PRED}}", &self.top_pred),
            ("{{LABEL_PRED}}", &self.label_pred),
            ("{{TIME_PRED}}", &self.time_pred),
            ("{{NUM_PRED}}", &self.num_pred),
            ("{{BASE_IRI}}", &self.base_iri),
            ("{{HUB_IRI}}", &self.hub_iri),
            ("{{BBOX_POLYGON}}", &self.bbox_polygon),
        ];
        for (ph, val) in pairs {
            if let Some(v) = val {
                out = out.replace(ph, v);
            }
        }
        out
    }
}

/// A CRS84 lon/lat WKT polygon for the bounding box, or the whole world when the
/// dataset has geometry but no wgs84 extent was derived.
fn bbox_polygon(bbox: Option<[f64; 4]>) -> String {
    let [min_lon, min_lat, max_lon, max_lat] = bbox.unwrap_or([-180.0, -90.0, 180.0, 90.0]);
    format!(
        "POLYGON(({min_lon} {min_lat}, {max_lon} {min_lat}, {max_lon} {max_lat}, {min_lon} {max_lat}, {min_lon} {min_lat}))"
    )
}

/// Generate the tiered starter-query library for a card: emit each template whose
/// required capabilities are all present, with placeholders substituted and the
/// shared PREFIX block prepended.
pub(crate) fn generate(card: &DatasetCard) -> Vec<ExampleQuery> {
    let caps = Caps::from_card(card);
    let mut out = Vec::new();
    for t in TEMPLATES {
        if !t.requires.iter().all(|&c| caps.available(c)) {
            continue;
        }
        let body = caps.substitute(t.body);
        debug_assert!(
            !body.contains("{{"),
            "template {} left an unsubstituted placeholder",
            t.id
        );
        out.push(ExampleQuery {
            id: t.id.to_string(),
            title: t.title.to_string(),
            dimension: t.dimension.to_string(),
            question: t.question.to_string(),
            sparql: format!("{PREFIXES}\n{body}"),
            tier: t.tier,
            requires: t.requires.iter().map(|c| c.key().to_string()).collect(),
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::card::{derive_card, CardInput};
    use rete_core::{eval_query, ingest, summary_query_shape, Rete};

    const TYPE: &str = "<http://www.w3.org/1999/02/22-rdf-syntax-ns#type>";

    fn q(s: &str, p: &str, o: &str) -> (String, String, String, Option<String>) {
        (s.into(), p.into(), o.into(), None)
    }

    /// A fixture exercising every signal so (almost) every template emits.
    fn rich_quads() -> Vec<(String, String, String, Option<String>)> {
        let label = "<http://www.w3.org/2000/01/rdf-schema#label>";
        let sameas = "<http://www.w3.org/2002/07/owl#sameAs>";
        let lat = "<http://www.w3.org/2003/01/geo/wgs84_pos#lat>";
        let long = "<http://www.w3.org/2003/01/geo/wgs84_pos#long>";
        let aswkt = "<http://www.opengis.net/ont/geosparql#asWKT>";
        let hasgeom = "<http://www.opengis.net/ont/geosparql#hasGeometry>";
        let gyear = "http://www.w3.org/2001/XMLSchema#gYear";
        let xint = "http://www.w3.org/2001/XMLSchema#integer";
        let wkt = "http://www.opengis.net/ont/geosparql#wktLiteral";
        let dbl = "http://www.w3.org/2001/XMLSchema#double";
        let mut v = Vec::new();
        for i in 0..6 {
            let s = format!("<http://ex/person/{i}>");
            v.push(q(&s, TYPE, "<http://ex/Person>"));
            v.push(q(&s, label, &format!("\"Person {i}\"@en")));
            v.push(q(&s, label, &format!("\"Personne {i}\"@fr")));
            v.push(q(
                &s,
                "<http://ex/birthYear>",
                &format!("\"{}\"^^<{gyear}>", 1800 + i),
            ));
            v.push(q(
                &s,
                "<http://ex/friendCount>",
                &format!("\"{}\"^^<{xint}>", i * 3),
            ));
            v.push(q(
                &s,
                "<http://ex/knows>",
                &format!("<http://ex/person/{}>", (i + 1) % 6),
            ));
            v.push(q(&s, "<http://ex/bornIn>", "<http://ex/place/0>"));
            if i % 2 == 0 {
                v.push(q(
                    &s,
                    sameas,
                    &format!("<http://www.wikidata.org/entity/Q{}>", 100 + i),
                ));
            }
        }
        v.push(q("<http://ex/place/0>", TYPE, "<http://ex/Place>"));
        v.push(q("<http://ex/place/0>", label, "\"Rome\"@en"));
        v.push(q("<http://ex/place/0>", lat, &format!("\"41.9\"^^<{dbl}>")));
        v.push(q(
            "<http://ex/place/0>",
            long,
            &format!("\"12.5\"^^<{dbl}>"),
        ));
        v.push(q(
            "<http://ex/place/0>",
            hasgeom,
            "<http://ex/place/0/geom>",
        ));
        v.push(q(
            "<http://ex/place/0/geom>",
            aswkt,
            &format!("\"POINT(12.5 41.9)\"^^<{wkt}>"),
        ));
        v
    }

    #[test]
    fn generated_queries_parse_run_and_are_tiered() {
        let quads = rich_quads();
        let card = derive_card(&quads, 50, 0, CardInput::default());
        assert!(
            card.queries.len() >= 12,
            "expected >= 12 starter queries, got {}",
            card.queries.len()
        );

        let (bytes, _) = ingest::assemble_dataset_with_opts(&quads, true, None, |_| Vec::new());
        let rete = Rete::open(&bytes).unwrap();

        for eq in &card.queries {
            assert!(
                eq.sparql.starts_with("PREFIX"),
                "{}: no PREFIX block",
                eq.id
            );
            assert!(
                !eq.sparql.contains("{{"),
                "{}: unsubstituted placeholder",
                eq.id
            );
            assert!(!eq.dimension.is_empty(), "{}: missing dimension", eq.id);

            // Parses AND runs against the real engine (>= 0 rows, never an error).
            let result = eval_query(&rete, &eq.sparql);
            assert!(
                result.is_ok(),
                "{}: failed to run: {:?}",
                eq.id,
                result.err()
            );

            // Every Summary-tagged query is recognized by the index-free summary
            // classifier — the tier is truthful, not aspirational.
            if eq.tier == Tier::Summary {
                let shape = summary_query_shape(&eq.sparql);
                assert!(
                    matches!(shape, Ok(Some(_))),
                    "{}: tagged Summary but not summary-shaped: {:?}",
                    eq.id,
                    shape
                );
            }
        }
    }

    #[test]
    fn families_are_gated_by_signal() {
        // A bare graph: no labels, no time, no geometry, no links, no named graphs.
        let quads = vec![
            q("<http://ex/a>", TYPE, "<http://ex/T>"),
            q("<http://ex/a>", "<http://ex/rel>", "<http://ex/b>"),
        ];
        let card = derive_card(&quads, 4, 0, CardInput::default());
        let ids: Vec<&str> = card.queries.iter().map(|q| q.id.as_str()).collect();

        // Signal-gated families are absent.
        for absent in [
            "sp-bbox",
            "sp-within",
            "ti-extent",
            "ti-histogram",
            "lb-labels",
            "ng-list",
        ] {
            assert!(!ids.contains(&absent), "{absent} should be gated out");
        }
        // Unconditional overview queries are still present.
        for present in ["ov-triples", "ov-pred-list", "ov-pred-hist"] {
            assert!(ids.contains(&present), "{present} should always emit");
        }
    }
}
