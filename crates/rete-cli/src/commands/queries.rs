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
//! Bodies are **graph-scope aware** ([`GraphScope`]): a dataset whose statements
//! live entirely in named graphs (`triple_count == 0`, `named_graph_count > 0`)
//! gets `GRAPH ?g`-scoped bodies — a bare `{ ?s ?p ?o }` there can only ever
//! return zero rows — and a dataset with data in **both** the default graph and
//! named graphs gets `UNION`-scoped overview bodies so neither half is silently
//! hidden.
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
    /// The default-graph body — used verbatim when the data lives in the
    /// default graph, so classic (graph-free) cards are byte-identical to
    /// before scope awareness existed.
    body: &'static str,
    /// Body when **every** statement lives in a named graph and the default
    /// graph is empty. On such a file the default-graph `body` can only return
    /// zero rows, so a template without a named-scope body is **skipped**
    /// (unless it is already `GRAPH`-native, i.e. requires [`Cap::NamedGraphs`]).
    named_body: Option<&'static str>,
    /// Body when **both** the default graph and named graphs hold data.
    /// `None` falls back to `body`: sound — the profile that instantiated it
    /// was derived from the (non-empty) default graph — but scoped to it, so
    /// the whole-dataset templates (counts, histograms, hubs) provide a
    /// `UNION`-scoped variant instead of silently hiding the named half.
    mixed_body: Option<&'static str>,
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
        named_body: Some("SELECT (COUNT(*) AS ?n) WHERE { GRAPH ?g { ?s ?p ?o } }"),
        mixed_body: Some(
            "SELECT (COUNT(*) AS ?n) WHERE { { ?s ?p ?o } UNION { GRAPH ?g { ?s ?p ?o } } }",
        ),
        tier: Tier::Summary,
        requires: &[],
    },
    Template {
        id: "ov-pred-list",
        title: "What relationships exist?",
        dimension: "overview",
        question: "Which predicates (relationships) appear in the data?",
        body: "SELECT DISTINCT ?p WHERE { ?s ?p ?o }",
        named_body: Some("SELECT DISTINCT ?p WHERE { GRAPH ?g { ?s ?p ?o } }"),
        mixed_body: Some(
            "SELECT DISTINCT ?p WHERE { { ?s ?p ?o } UNION { GRAPH ?g { ?s ?p ?o } } }",
        ),
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
        named_body: Some("SELECT ?p (COUNT(*) AS ?n) WHERE { GRAPH ?g { ?s ?p ?o } } GROUP BY ?p"),
        mixed_body: Some(
            "SELECT ?p (COUNT(*) AS ?n) WHERE { { ?s ?p ?o } UNION { GRAPH ?g { ?s ?p ?o } } } GROUP BY ?p",
        ),
        tier: Tier::Summary,
        requires: &[],
    },
    Template {
        id: "ov-ask-pred",
        title: "Does the top predicate occur?",
        dimension: "overview",
        question: "Is the most common predicate present at all?",
        body: "ASK { ?s {{TOP_PRED}} ?o }",
        named_body: Some("ASK { GRAPH ?g { ?s {{TOP_PRED}} ?o } }"),
        mixed_body: None, // the profiled predicate lives in the default graph
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
        named_body: None,
        mixed_body: None,
        tier: Tier::Index,
        requires: &[Cap::TopClass],
    },
    Template {
        id: "lb-labels",
        title: "Readable names for entities",
        dimension: "labels",
        question: "What are the human-readable names of some entities?",
        body: "SELECT ?s ?label WHERE { ?s a {{TOP_CLASS}} ; {{LABEL_PRED}} ?label } LIMIT 50",
        named_body: None,
        mixed_body: None,
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
        named_body: None,
        mixed_body: None,
        tier: Tier::Index,
        requires: &[Cap::TopClass],
    },
    Template {
        id: "ty-entities",
        title: "How many distinct typed entities?",
        dimension: "types",
        question: "How many distinct entities carry a type?",
        body: "SELECT (COUNT(DISTINCT ?s) AS ?n) WHERE { ?s a [] }",
        named_body: None,
        mixed_body: None,
        tier: Tier::Index,
        requires: &[Cap::TopClass],
    },
    Template {
        id: "ty-class-shape",
        title: "What does the top class look like?",
        dimension: "types",
        question: "Which predicates describe the most common class?",
        body: "SELECT ?p (COUNT(*) AS ?n) WHERE { ?s a {{TOP_CLASS}} ; ?p ?o } GROUP BY ?p ORDER BY DESC(?n)",
        named_body: None,
        mixed_body: None,
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
        named_body: None,
        mixed_body: None,
        tier: Tier::Index,
        requires: &[Cap::TopClass],
    },
    Template {
        id: "top-out-hubs",
        title: "Most connected sources",
        dimension: "topology",
        question: "Which subjects make the most statements?",
        body: "SELECT ?s (COUNT(*) AS ?d) WHERE { ?s ?p ?o } GROUP BY ?s ORDER BY DESC(?d) LIMIT 25",
        named_body: Some(
            "SELECT ?s (COUNT(*) AS ?d) WHERE { GRAPH ?g { ?s ?p ?o } } GROUP BY ?s ORDER BY DESC(?d) LIMIT 25",
        ),
        mixed_body: Some(
            "SELECT ?s (COUNT(*) AS ?d) WHERE { { ?s ?p ?o } UNION { GRAPH ?g { ?s ?p ?o } } } GROUP BY ?s ORDER BY DESC(?d) LIMIT 25",
        ),
        tier: Tier::Index,
        requires: &[],
    },
    Template {
        id: "top-in-hubs",
        title: "Most referenced entities",
        dimension: "topology",
        question: "Which resources are referenced most often?",
        body: "SELECT ?o (COUNT(*) AS ?d) WHERE { ?s ?p ?o FILTER(!isLiteral(?o)) } GROUP BY ?o ORDER BY DESC(?d) LIMIT 25",
        named_body: Some(
            "SELECT ?o (COUNT(*) AS ?d) WHERE { GRAPH ?g { ?s ?p ?o FILTER(!isLiteral(?o)) } } GROUP BY ?o ORDER BY DESC(?d) LIMIT 25",
        ),
        mixed_body: Some(
            "SELECT ?o (COUNT(*) AS ?d) WHERE { { ?s ?p ?o } UNION { GRAPH ?g { ?s ?p ?o } } FILTER(!isLiteral(?o)) } GROUP BY ?o ORDER BY DESC(?d) LIMIT 25",
        ),
        tier: Tier::Index,
        requires: &[],
    },
    Template {
        id: "top-dangling",
        title: "Referenced but undescribed IRIs",
        dimension: "topology",
        question: "Which IRIs are referenced as objects but never described?",
        body: "SELECT ?o WHERE { ?s ?p ?o FILTER(isIRI(?o)) FILTER NOT EXISTS { ?o ?p2 ?o2 } } LIMIT 100",
        named_body: Some(
            "SELECT ?o WHERE { GRAPH ?g { ?s ?p ?o FILTER(isIRI(?o)) } FILTER NOT EXISTS { GRAPH ?h { ?o ?p2 ?o2 } } } LIMIT 100",
        ),
        mixed_body: Some(
            "SELECT ?o WHERE { { ?s ?p ?o } UNION { GRAPH ?g { ?s ?p ?o } } FILTER(isIRI(?o)) FILTER NOT EXISTS { ?o ?p2 ?o2 } FILTER NOT EXISTS { GRAPH ?h { ?o ?p3 ?o3 } } } LIMIT 100",
        ),
        tier: Tier::Index,
        requires: &[],
    },
    Template {
        id: "top-reach",
        title: "What's reachable from a hub?",
        dimension: "connectivity",
        question: "What can you reach from the busiest hub via the top predicate?",
        body: "SELECT ?y WHERE { {{HUB_IRI}} {{TOP_PRED}}+ ?y } LIMIT 100",
        named_body: None,
        mixed_body: None,
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
        named_body: None,
        mixed_body: None,
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
        named_body: None,
        mixed_body: None,
        tier: Tier::Index,
        requires: &[Cap::Link],
    },
    Template {
        id: "lk-external",
        title: "Which predicates link out?",
        dimension: "links",
        question: "Which predicates point to IRIs outside this dataset?",
        body: "SELECT ?p (COUNT(*) AS ?n) WHERE { ?s ?p ?o FILTER(isIRI(?o) && !STRSTARTS(STR(?o), \"{{BASE_IRI}}\")) } GROUP BY ?p ORDER BY DESC(?n) LIMIT 50",
        named_body: None,
        mixed_body: None,
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
        named_body: None,
        mixed_body: None,
        tier: Tier::Index,
        requires: &[Cap::HasLiterals],
    },
    Template {
        id: "lt-langs",
        title: "What languages?",
        dimension: "literals",
        question: "Which language tags appear on literals, and how often?",
        body: "SELECT ?l (COUNT(*) AS ?n) WHERE { ?s ?p ?o FILTER(isLiteral(?o)) } GROUP BY (LANG(?o) AS ?l) ORDER BY DESC(?n)",
        named_body: None,
        mixed_body: None,
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
        named_body: None,
        mixed_body: None,
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
        named_body: None,
        mixed_body: None,
        tier: Tier::Index,
        requires: &[Cap::TimePred],
    },
    Template {
        id: "ti-histogram",
        title: "Distribution over time",
        dimension: "time",
        question: "How are entities distributed by year?",
        body: "SELECT ?yr (COUNT(*) AS ?n) WHERE { ?s {{TIME_PRED}} ?d BIND(SUBSTR(STR(?d),1,4) AS ?yr) } GROUP BY ?yr ORDER BY ?yr",
        named_body: None,
        mixed_body: None,
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
        named_body: None,
        mixed_body: None,
        tier: Tier::Index,
        requires: &[Cap::GeoLatLong],
    },
    Template {
        id: "sp-within",
        title: "Features inside a box",
        dimension: "space",
        question: "Which features fall inside the dataset's bounding box? (WKT is lon/lat)",
        body: "SELECT ?s WHERE { ?s geo:hasGeometry/geo:asWKT ?w FILTER(geof:sfWithin(?w, \"{{BBOX_POLYGON}}\"^^geo:wktLiteral)) } LIMIT 100",
        named_body: None,
        mixed_body: None,
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
        named_body: None,
        mixed_body: None,
        tier: Tier::Index,
        requires: &[Cap::NamedGraphs],
    },
    Template {
        id: "ng-sizes",
        title: "Which graphs are biggest?",
        dimension: "graphs",
        question: "Which named graphs hold the most statements?",
        // LIMIT keeps this a starter on a file with tens of thousands of graphs.
        body: "SELECT ?g (COUNT(*) AS ?n) WHERE { GRAPH ?g { ?s ?p ?o } } GROUP BY ?g ORDER BY DESC(?n) LIMIT 25",
        named_body: None,
        mixed_body: None,
        tier: Tier::Index,
        requires: &[Cap::NamedGraphs],
    },
    Template {
        id: "ng-sample",
        title: "A peek inside the graphs",
        dimension: "graphs",
        question: "What do statements in the named graphs look like?",
        // `?g` in the projection on purpose: each sample row says where it lives.
        body: "SELECT ?g ?s ?p ?o WHERE { GRAPH ?g { ?s ?p ?o } } LIMIT 10",
        named_body: None,
        mixed_body: None,
        tier: Tier::Index,
        requires: &[Cap::NamedGraphs],
    },
];

/// Where the dataset's statements live, decided from the card's own counts —
/// so the emitted queries address the graph(s) that actually hold data instead
/// of scanning a default graph the card itself records as empty.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum GraphScope {
    /// No named graphs — bodies are used verbatim, so classic cards are
    /// byte-identical to pre-scope-awareness builds.
    DefaultOnly,
    /// Every statement lives in a named graph and the default graph is empty
    /// (`triple_count == 0 && named_graph_count > 0`) — a bare `{ ?s ?p ?o }`
    /// can only ever return zero rows here.
    NamedOnly,
    /// Both the default graph and named graphs hold statements.
    Mixed,
}

impl GraphScope {
    fn of(card: &DatasetCard) -> Self {
        if card.named_graph_count == 0 {
            GraphScope::DefaultOnly
        } else if card.triple_count == 0 {
            GraphScope::NamedOnly
        } else {
            GraphScope::Mixed
        }
    }
}

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
/// required capabilities are all present, with the body picked for where the data
/// lives ([`GraphScope`]), placeholders substituted, and the shared PREFIX block
/// prepended.
pub(crate) fn generate(card: &DatasetCard) -> Vec<ExampleQuery> {
    let caps = Caps::from_card(card);
    let scope = GraphScope::of(card);
    let mut out = Vec::new();
    for t in TEMPLATES {
        if !t.requires.iter().all(|&c| caps.available(c)) {
            continue;
        }
        // Pick the body for where the data lives. A scope-variant body is
        // always Index-tier: GRAPH/UNION patterns are not summary-shaped
        // (`summary_query_shape` recognizes only a bare default-graph
        // pattern), and the pyramid summary covers the default graph only.
        let (body, tier) = match scope {
            GraphScope::DefaultOnly => (t.body, t.tier),
            GraphScope::NamedOnly => match t.named_body {
                Some(b) => (b, Tier::Index),
                // GRAPH-native templates (the ng-* family) already address the
                // named graphs; any other default-graph body would be a
                // guaranteed-zero-rows query on a file whose default graph the
                // card itself records as empty — skip it.
                None if t.requires.contains(&Cap::NamedGraphs) => (t.body, t.tier),
                None => continue,
            },
            GraphScope::Mixed => match t.mixed_body {
                Some(b) => (b, Tier::Index),
                // The default-graph body is sound here — the profile that
                // instantiated it was derived from the (non-empty) default
                // graph — and the ng-* family surfaces the named half.
                None => (t.body, t.tier),
            },
        };
        let body = caps.substitute(body);
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
            tier,
            requires: t.requires.iter().map(|c| c.key().to_string()).collect(),
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::card::{derive_card, CardInput};
    use rete_core::{eval_query, ingest, summary_query_shape, QueryOutput, Rete};

    const TYPE: &str = "<http://www.w3.org/1999/02/22-rdf-syntax-ns#type>";

    fn q(s: &str, p: &str, o: &str) -> (String, String, String, Option<String>) {
        (s.into(), p.into(), o.into(), None)
    }

    /// The property that would have caught the named-graph-only card bug:
    /// every query the card ships **returns a non-empty result** when run
    /// against the very graph it was generated for.
    fn assert_every_query_returns_rows(card: &DatasetCard, rete: &Rete) {
        assert!(!card.queries.is_empty(), "card generated no queries at all");
        for eq in &card.queries {
            match eval_query(rete, &eq.sparql) {
                Ok(QueryOutput::Select(_, rows)) => {
                    assert!(!rows.is_empty(), "{}: returned zero rows", eq.id)
                }
                Ok(QueryOutput::Ask(b)) => assert!(b, "{}: ASK returned false", eq.id),
                Ok(QueryOutput::Construct(ts)) => {
                    assert!(!ts.is_empty(), "{}: constructed nothing", eq.id)
                }
                // `QueryOutput` is non-exhaustive; the library only emits the
                // forms above.
                Ok(other) => panic!("{}: unexpected result form: {other:?}", eq.id),
                Err(e) => panic!("{}: failed to run: {e:?}", eq.id),
            }
        }
    }

    /// The single COUNT value of an aggregate query's one-row result.
    fn count_value(rete: &Rete, sparql: &str, var: &str) -> u64 {
        match eval_query(rete, sparql).unwrap() {
            QueryOutput::Select(_, rows) => {
                assert_eq!(rows.len(), 1, "expected one aggregate row");
                let lit = rows[0].get(var).expect("aggregate variable bound");
                lit.trim_start_matches('"')
                    .split('"')
                    .next()
                    .unwrap()
                    .parse()
                    .expect("count literal parses")
            }
            other => panic!("expected a SELECT result, got {other:?}"),
        }
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

        let (bytes, _) =
            ingest::assemble_dataset_with_opts(quads, true, false, None, |_, _| Vec::new());
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

    /// Every statement in a named graph, the default graph empty — the shape
    /// (e.g. nkod.rete) whose cards used to ship six guaranteed-zero-rows
    /// queries. Includes an IRI that is referenced but never described, so
    /// `top-dangling` has rows to find.
    fn named_only_quads() -> Vec<(String, String, String, Option<String>)> {
        let mut v = Vec::new();
        for gi in 0..3 {
            let g = format!("<http://ex/graph/{gi}>");
            for i in 0..4 {
                let s = format!("<http://ex/item/{gi}-{i}>");
                v.push((
                    s.clone(),
                    TYPE.to_string(),
                    "<http://ex/Item>".to_string(),
                    Some(g.clone()),
                ));
                v.push((
                    s.clone(),
                    "<http://ex/rel>".to_string(),
                    format!("<http://ex/item/{gi}-{}>", (i + 1) % 4),
                    Some(g.clone()),
                ));
                v.push((
                    s,
                    "<http://ex/ref>".to_string(),
                    "<http://other/never-described>".to_string(),
                    Some(g.clone()),
                ));
            }
        }
        v
    }

    #[test]
    fn named_graph_only_queries_are_scoped_and_return_rows() {
        let quads = named_only_quads();
        let total = quads.len() as u64;
        let card = derive_card(&quads, 20, 3, CardInput::default());
        // The precondition of the bug: the card itself knows the default graph
        // is empty and the data lives in named graphs.
        assert_eq!(card.triple_count, 0);
        assert_eq!(card.named_graph_count, 3);

        // No emitted query may scan only the (empty) default graph.
        for eq in &card.queries {
            assert!(
                eq.sparql.contains("GRAPH"),
                "{}: default-graph-only query on a named-graph-only file:\n{}",
                eq.id,
                eq.sparql
            );
            // The pyramid summary covers the default graph only, so no
            // GRAPH-scoped query may claim the Summary tier.
            assert_ne!(eq.tier, Tier::Summary, "{}: untruthful Summary tier", eq.id);
        }

        let (bytes, _) =
            ingest::assemble_dataset_with_opts(quads, true, false, None, |_, _| Vec::new());
        let rete = Rete::open(&bytes).unwrap();
        assert_every_query_returns_rows(&card, &rete);

        // The headline count now reflects the data, not the empty default graph.
        let ov = card.queries.iter().find(|q| q.id == "ov-triples").unwrap();
        assert_eq!(count_value(&rete, &ov.sparql, "n"), total);
    }

    #[test]
    fn mixed_graph_queries_cover_both_sides_and_return_rows() {
        // Data in BOTH the default graph and a named graph.
        let mut quads = Vec::new();
        let label = "<http://www.w3.org/2000/01/rdf-schema#label>";
        for i in 0..4 {
            let s = format!("<http://ex/doc/{i}>");
            quads.push(q(&s, TYPE, "<http://ex/Doc>"));
            quads.push(q(&s, label, &format!("\"Doc {i}\"@en")));
            quads.push(q(
                &s,
                "<http://ex/rel>",
                &format!("<http://ex/doc/{}>", (i + 1) % 4),
            ));
            quads.push(q(&s, "<http://ex/rel>", "<http://other/never-described>"));
        }
        let default_count = quads.len() as u64;
        let g = "<http://ex/graph/annotations>".to_string();
        for i in 0..5 {
            quads.push((
                format!("<http://ex/note/{i}>"),
                "<http://ex/about>".to_string(),
                format!("<http://ex/doc/{}>", i % 4),
                Some(g.clone()),
            ));
        }
        let total = quads.len() as u64;

        let card = derive_card(&quads, 30, 1, CardInput::default());
        assert_eq!(card.triple_count, default_count);
        assert_eq!(card.quad_count, total);
        assert_eq!(card.named_graph_count, 1);

        let (bytes, _) =
            ingest::assemble_dataset_with_opts(quads, true, false, None, |_, _| Vec::new());
        let rete = Rete::open(&bytes).unwrap();
        assert_every_query_returns_rows(&card, &rete);

        // The overview count covers BOTH sides — neither half silently hidden.
        let ov = card.queries.iter().find(|q| q.id == "ov-triples").unwrap();
        assert_eq!(count_value(&rete, &ov.sparql, "n"), total);
        // And the named-graph family is present alongside the profile queries.
        let ids: Vec<&str> = card.queries.iter().map(|q| q.id.as_str()).collect();
        for id in ["ng-list", "ng-sizes", "ng-sample", "id-sample", "lb-labels"] {
            assert!(ids.contains(&id), "{id} missing from a mixed-graph card");
        }
    }

    #[test]
    fn default_graph_bodies_are_unchanged() {
        // A graph with no named graphs must get the classic bodies verbatim —
        // scope awareness may not perturb existing default-graph cards.
        let quads = rich_quads();
        let card = derive_card(&quads, 50, 0, CardInput::default());
        let ov = card.queries.iter().find(|q| q.id == "ov-triples").unwrap();
        assert!(ov
            .sparql
            .ends_with("SELECT (COUNT(*) AS ?n) WHERE { ?s ?p ?o }"));
        assert_eq!(ov.tier, Tier::Summary);
        for eq in &card.queries {
            assert!(
                !eq.sparql.contains("GRAPH ?g") && !eq.sparql.contains("UNION"),
                "{}: graph-scoped body leaked into a default-graph card",
                eq.id
            );
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
            "ng-sample",
        ] {
            assert!(!ids.contains(&absent), "{absent} should be gated out");
        }
        // Unconditional overview queries are still present.
        for present in ["ov-triples", "ov-pred-list", "ov-pred-hist"] {
            assert!(ids.contains(&present), "{present} should always emit");
        }
    }
}
