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
//! # Why a generated query may not return zero rows
//!
//! A starter query that answers nothing is worse than no starter query: the
//! reader concludes the **file** is broken. So every template carries a
//! [`NonEmpty`] claim — the reason its emitted form cannot come back empty —
//! and [`check_body`] enforces the one rule that claim rests on:
//!
//! > A body may conjoin substituted vocabulary only when the card **proves the
//! > pieces co-occur**.
//!
//! Presence is not co-occurrence. `{{TOP_CLASS}}` is the class with the most
//! instances and `{{LABEL_PRED}}` the most-used labelling predicate; each is
//! certainly *present*, and instances of that class need never carry that
//! predicate. Conjoining them — which `lb-labels` did — is a guaranteed-empty
//! query whenever the two peaks fall on different parts of the graph, as they
//! do on `mtg` (top class `mtg:Ruling`, which has no `schema:name`) and on
//! `hugging-face` (top class `hf:Model`; `rdfs:label` appears only on the
//! embedded TBox terms). The fix is not to check harder downstream but to make
//! the conjunction unrepresentable: capabilities that appear together in a body
//! must be **jointly derived** ([`Cap::joint_with`]) — chosen *because* they
//! meet, with a `class_links` row as the witness.
//!
//! Where the card genuinely cannot decide emptiness, the template says so
//! ([`NonEmpty::Undecidable`]) instead of pretending; `provably_empty` is the
//! last gate, dropping a template the card can prove would answer nothing.
//!
//! This is pure CLI/serde generation — no format change.

use super::card::{DatasetCard, ExampleQuery, Tier, GEO_ASWKT, GEO_HASGEOMETRY, O_LITERAL};
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
    /// The most populous class the card can **prove** carries [`Cap::LabelPred`].
    LabeledClass,
    LabelPred,
    /// The most frequent predicate the card can **prove** has a non-literal
    /// object — the only kind a path query can walk past one hop.
    ObjectPred,
    TimePred,
    NumPred,
    BaseIri,
    /// An IRI the card records that lies **outside** [`Cap::BaseIri`] — the
    /// witness that "links pointing out of this dataset" has something to find.
    ExternalIri,
    GeoLatLong,
    /// The path from a subject to its WKT geometry, as the data actually shapes
    /// it (`geo:asWKT` direct, or through `geo:hasGeometry`).
    WktPath,
    /// The query box for the geometry template.
    Bbox,
    NamedGraphs,
    Link,
    HasLiterals,
}

/// Every capability, in declaration order — the substitution table and the
/// placeholder audit both walk it, so a new `Cap` cannot be half-wired.
const ALL_CAPS: &[Cap] = &[
    Cap::TopClass,
    Cap::TopPred,
    Cap::LabeledClass,
    Cap::LabelPred,
    Cap::ObjectPred,
    Cap::TimePred,
    Cap::NumPred,
    Cap::BaseIri,
    Cap::ExternalIri,
    Cap::GeoLatLong,
    Cap::WktPath,
    Cap::Bbox,
    Cap::NamedGraphs,
    Cap::Link,
    Cap::HasLiterals,
];

impl Cap {
    /// Stable key recorded in `ExampleQuery.requires`.
    fn key(self) -> &'static str {
        match self {
            Cap::TopClass => "TOP_CLASS",
            Cap::TopPred => "TOP_PRED",
            Cap::LabeledClass => "LABELED_CLASS",
            Cap::LabelPred => "LABEL_PRED",
            Cap::ObjectPred => "OBJECT_PRED",
            Cap::TimePred => "TIME_PRED",
            Cap::NumPred => "NUM_PRED",
            Cap::BaseIri => "BASE_IRI",
            Cap::ExternalIri => "EXTERNAL_IRI",
            Cap::GeoLatLong => "GEO_LATLONG",
            Cap::WktPath => "WKT_PATH",
            Cap::Bbox => "BBOX",
            Cap::NamedGraphs => "NAMED_GRAPHS",
            Cap::Link => "LINK",
            Cap::HasLiterals => "HAS_LITERALS",
        }
    }

    /// The `{{PLACEHOLDER}}` this capability's value is substituted into, for
    /// the string-valued ones. `None` for the pure gates (they have no text in
    /// the body — they only decide whether it is emitted).
    fn placeholder(self) -> Option<&'static str> {
        Some(match self {
            Cap::TopClass => "{{TOP_CLASS}}",
            Cap::TopPred => "{{TOP_PRED}}",
            Cap::LabeledClass => "{{LABELED_CLASS}}",
            Cap::LabelPred => "{{LABEL_PRED}}",
            Cap::ObjectPred => "{{OBJECT_PRED}}",
            Cap::TimePred => "{{TIME_PRED}}",
            Cap::NumPred => "{{NUM_PRED}}",
            Cap::BaseIri => "{{BASE_IRI}}",
            Cap::WktPath => "{{WKT_PATH}}",
            Cap::Bbox => "{{BBOX_POLYGON}}",
            Cap::ExternalIri
            | Cap::GeoLatLong
            | Cap::NamedGraphs
            | Cap::Link
            | Cap::HasLiterals => return None,
        })
    }

    /// The capabilities this one is chosen **together with** — the card holds a
    /// `class_links` row proving they meet on the same subjects, so a body may
    /// conjoin them.
    ///
    /// This is the whole safety property. Every other pair of substitutions is
    /// picked from a different ranking (most instances / most statements / most
    /// labels), and two independent maxima need not describe the same entity;
    /// see the module docs. The relation is symmetric — [`check_body`] checks
    /// both directions.
    fn joint_with(self) -> &'static [Cap] {
        match self {
            // `LABELED_CLASS` *is* "the class that carries `LABEL_PRED`". The
            // one pair the card can witness, and the one the label queries need.
            Cap::LabeledClass => &[Cap::LabelPred],
            Cap::LabelPred => &[Cap::LabeledClass],
            _ => &[],
        }
    }
}

/// Why a template's emitted query cannot come back empty. Declared per template
/// and enforced by [`check_body`] plus the fixture tests, which run every
/// generated query against the very graph it was generated for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NonEmpty {
    /// Any non-empty graph answers: the body has no substitution and no filter
    /// that could exclude every statement.
    AnyGraph,
    /// An aggregate with no `GROUP BY` — SPARQL returns exactly one row even
    /// over an empty solution sequence, so "zero rows" is not reachable. (The
    /// row's values may still be unbound; see each template's note.)
    Aggregate,
    /// Every substitution came from a count the card recorded as `> 0`, and any
    /// two that meet in the body are [jointly derived](Cap::joint_with).
    Witnessed,
    /// The card cannot decide it. The string is the reason, and it is the text
    /// a reader deserves when the query does answer nothing.
    Undecidable(&'static str),
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
    /// Why the emitted query cannot return zero rows.
    nonempty: NonEmpty,
    /// A weaker body for when `requires` is **not** met: the template degrades
    /// to a still-answerable question instead of vanishing. Without it a
    /// capability that only sharpens a query (`LABELED_CLASS`) would also be
    /// able to delete it — on `hugging-face` the label query would disappear
    /// even though the dataset has labels, which is not an improvement over
    /// shipping one that returns nothing.
    fallback: Option<Fallback>,
    /// Decides emptiness from the card, in **default-graph scope only** (the
    /// card profiles the default graph, so its counts witness nothing about a
    /// `GRAPH`-scoped body). `Some(f)` and `f(card)` true ⇒ the query would
    /// answer nothing ⇒ it is not emitted. The generator's last gate.
    provably_empty: Option<fn(&DatasetCard) -> bool>,
}

/// The reduced form of a [`Template`], used when its full `requires` are not
/// available. Default-graph shaped (no scope variants), so it is skipped on a
/// named-graph-only file exactly like a template without a `named_body`.
struct Fallback {
    body: &'static str,
    requires: &'static [Cap],
    nonempty: NonEmpty,
}

/// The library. Order is fixed → deterministic output (folded into the hash).
const TEMPLATES: &[Template] = &[
    // --- Overview ---
    Template {
        id: "ov-one-row",
        title: "One statement, now",
        dimension: "overview",
        question: "Return exactly one statement — did this file open and answer?",
        // The smoke test (issue #153): guaranteed one row on ANY non-empty
        // file. A COUNT can honestly return 0 (named-graph-only files) and
        // read as failure; one concrete row is unambiguous.
        body: "SELECT ?s ?p ?o WHERE { ?s ?p ?o } LIMIT 1",
        named_body: Some("SELECT ?g ?s ?p ?o WHERE { GRAPH ?g { ?s ?p ?o } } LIMIT 1"),
        mixed_body: Some(
            "SELECT ?s ?p ?o WHERE { { ?s ?p ?o } UNION { GRAPH ?g { ?s ?p ?o } } } LIMIT 1",
        ),
        tier: Tier::Index,
        requires: &[],
        nonempty: NonEmpty::AnyGraph,
        fallback: None,
        provably_empty: None,
    },
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
        nonempty: NonEmpty::Aggregate,
        fallback: None,
        provably_empty: None,
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
        nonempty: NonEmpty::AnyGraph,
        fallback: None,
        provably_empty: None,
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
        // A GROUP BY yields no groups over an empty sequence — but every
        // statement has a predicate, so a non-empty graph has at least one.
        nonempty: NonEmpty::AnyGraph,
        fallback: None,
        provably_empty: None,
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
        // One substitution, from a predicate the card counted; nothing to
        // conjoin it with, so the ASK cannot be false.
        nonempty: NonEmpty::Witnessed,
        fallback: None,
        provably_empty: None,
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
        nonempty: NonEmpty::Witnessed,
        fallback: None,
        provably_empty: None,
    },
    Template {
        id: "lb-labels",
        title: "Readable names for entities",
        dimension: "labels",
        question: "What are the human-readable names of some entities?",
        //
        // The class is `{{LABELED_CLASS}}`, NOT `{{TOP_CLASS}}`. Both name a
        // class the dataset certainly has, and only one of them is proven to
        // carry the label predicate this body joins it to. Picking the two
        // maxima independently — the class with the most instances, the
        // predicate with the most uses — ships a guaranteed-empty query
        // whenever they fall on different parts of the graph:
        //
        //   mtg           top class `mtg:Ruling` (76,990), which has no
        //                 `schema:name` at all — 0 rows, on a plain
        //                 default-graph file.
        //   hugging-face  top class `hf:Model` (2.9 M); `rdfs:label` occurs
        //                 only on the 64 embedded ontology terms — 0 rows.
        //
        // `LABELED_CLASS` walks `card.classes` in descending instance order and
        // takes the first one a `class_links` row proves carries the predicate.
        // That row is a positive witness (some subject has both statements), so
        // the join cannot be empty — and on a dataset where the top class *is*
        // labelled (the common case: geoadmin, lombardi) it resolves to exactly
        // the top class, leaving those cards byte-identical.
        //
        // Two other shapes were considered:
        //
        //   Drop the class (`?s {{LABEL_PRED}} ?label`). Simplest, and it also
        //   cannot fail — but it answers a strictly weaker question, throws away
        //   the join that is the single most reusable pattern a newcomer copies
        //   out of a starter query, and would rewrite the query on every card
        //   that is already correct.
        //
        //   Emit only when the top class is labelled. Silent and lossy: a
        //   dataset whose labels live off the top class loses its label query
        //   entirely, which is how a reader concludes there are no labels.
        //
        // So: prefer the typed form when the card can prove it, and fall back to
        // the class-free form when it cannot — the case where `class_links` (top
        // 100 rows) truncated the witness away, or where labels sit on untyped
        // subjects. `hugging-face` takes the fallback and answers with 50 rows.
        body: "SELECT ?s ?label WHERE { ?s a {{LABELED_CLASS}} ; {{LABEL_PRED}} ?label } LIMIT 50",
        named_body: None,
        mixed_body: None,
        tier: Tier::Index,
        requires: &[Cap::LabeledClass, Cap::LabelPred],
        nonempty: NonEmpty::Witnessed,
        fallback: Some(Fallback {
            body: "SELECT ?s ?label WHERE { ?s {{LABEL_PRED}} ?label } LIMIT 50",
            requires: &[Cap::LabelPred],
            nonempty: NonEmpty::Witnessed,
        }),
        provably_empty: None,
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
        // Gated on a class existing, and the body substitutes nothing.
        nonempty: NonEmpty::Witnessed,
        fallback: None,
        provably_empty: None,
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
        nonempty: NonEmpty::Aggregate,
        fallback: None,
        provably_empty: None,
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
        // One substitution, conjoined only with a wholly-variable pattern that
        // the class assertion itself already satisfies (`?p ?o` matches the
        // `rdf:type` statement) — so this is not the two-maxima shape.
        nonempty: NonEmpty::Witnessed,
        fallback: None,
        provably_empty: None,
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
        nonempty: NonEmpty::Witnessed,
        fallback: None,
        provably_empty: None,
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
        nonempty: NonEmpty::AnyGraph,
        fallback: None,
        provably_empty: None,
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
        // A graph whose objects are ALL literals (a flat table of values, no
        // `rdf:type` anywhere) has no group to return. `card.in_hubs` is
        // exactly this query's precompute, so in default-graph scope the
        // emptiness is decidable — see `provably_empty`. In `GRAPH` scope the
        // card profiles the wrong half and cannot say.
        nonempty: NonEmpty::Undecidable(
            "no non-literal object exists; decidable (and refused) in default-graph scope only, \
             since the card profiles the default graph and this body may be GRAPH-scoped",
        ),
        fallback: None,
        provably_empty: Some(|card| card.in_hubs.is_empty()),
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
        // The honest one: a fully-described graph legitimately has none, and
        // "is every referenced IRI described?" is precisely what the card does
        // not record. Only the trivial half is decidable (no IRI object at all).
        nonempty: NonEmpty::Undecidable(
            "a fully-described graph has no dangling IRI — emptiness here is the answer, \
             not a failure, and the card does not record which objects are also subjects",
        ),
        fallback: None,
        provably_empty: Some(|card| card.in_hubs.is_empty()),
    },
    Template {
        id: "top-reach",
        title: "What's reachable by following one relation?",
        dimension: "connectivity",
        question: "Following the most common entity-to-entity relation, what is reachable from a node?",
        //
        // Was `{{HUB_IRI}} {{TOP_PRED}}+ ?y`: the busiest subject and the most
        // frequent predicate, chosen independently and then conjoined — the
        // same defect as `lb-labels`, and live. On `hugging-face` the hub is a
        // User and the top predicate `schema:keywords` (which only Models
        // carry): 0 rows. Nothing in the card ties a specific subject to a
        // specific predicate, so that pair can never be witnessed and the seed
        // has to come from the relation itself.
        //
        // The sub-SELECT picks one real subject OF the relation, so the outer
        // path always has at least the seed's own edge to return; the seed stays
        // a single bound term, so the traversal costs what it did before (a
        // bare `?x p+ ?y` would enumerate the whole closure, and these queries
        // are measured at build time).
        //
        // `OBJECT_PRED`, not `TOP_PRED`: the most frequent predicate is usually
        // a labelling one, whose objects are literals — a `+` over it can never
        // walk past one hop. A `class_links` row with a non-literal `o_class`
        // proves the relation goes entity-to-entity. A self-referential one
        // (`s_class == o_class`) would chain deeper still, but its closure is
        // unbounded — `hf:follows` is 886 k edges of social graph — so
        // frequency, not recursion, picks the winner.
        body: "SELECT ?x ?y WHERE { { SELECT ?x WHERE { ?x {{OBJECT_PRED}} ?o } LIMIT 1 } ?x {{OBJECT_PRED}}+ ?y } LIMIT 100",
        named_body: None,
        mixed_body: None,
        tier: Tier::Index,
        requires: &[Cap::ObjectPred],
        nonempty: NonEmpty::Witnessed,
        fallback: None,
        provably_empty: None,
    },
    // --- Connectivity / completeness ---
    Template {
        id: "cmp-coverage",
        title: "Do all entities of the labelled class have a label?",
        dimension: "connectivity",
        question: "How complete is labelling on the class that carries labels?",
        // `{{LABELED_CLASS}}` for the same reason as `lb-labels`, plus one of
        // its own: with `{{TOP_CLASS}}` this reported `76990 / 0` on mtg — a
        // row, so never caught by a zero-rows gate, and a measurement of the
        // wrong class. Pairing it with `lb-labels` also keeps the two label
        // queries talking about the same entities.
        body: "SELECT (COUNT(?s) AS ?total) (COUNT(?l) AS ?have) WHERE { ?s a {{LABELED_CLASS}} OPTIONAL { ?s {{LABEL_PRED}} ?l } }",
        named_body: None,
        mixed_body: None,
        tier: Tier::Index,
        requires: &[Cap::LabeledClass, Cap::LabelPred],
        nonempty: NonEmpty::Aggregate,
        fallback: None,
        provably_empty: None,
    },
    // --- Links ---
    Template {
        id: "lk-sameas",
        title: "Aligned to which external datasets?",
        dimension: "links",
        question: "Which entity-alignment predicates are used, and how often?",
        // The VALUES list must cover the gate exactly. It listed three of the
        // four predicates `signals.link_predicates` is detected from, so a
        // dataset aligned only with `skos:closeMatch` passed the gate and then
        // grouped nothing. Adding the fourth makes emptiness unreachable.
        body: "SELECT ?p (COUNT(*) AS ?n) WHERE { ?s ?p ?o VALUES ?p { owl:sameAs skos:exactMatch skos:closeMatch rdfs:seeAlso } } GROUP BY ?p",
        named_body: None,
        mixed_body: None,
        tier: Tier::Index,
        requires: &[Cap::Link],
        nonempty: NonEmpty::Witnessed,
        fallback: None,
        provably_empty: None,
    },
    Template {
        id: "lk-external",
        title: "Which predicates link out?",
        dimension: "links",
        question: "Which predicates point to IRIs outside this dataset?",
        // `EXTERNAL_IRI` is the witness that the FILTER keeps something: an IRI
        // in `classes` or `in_hubs` that does not start with the base IRI.
        // Without it a self-contained dataset — every object either a literal
        // or an in-namespace IRI — emitted a query that grouped nothing.
        body: "SELECT ?p (COUNT(*) AS ?n) WHERE { ?s ?p ?o FILTER(isIRI(?o) && !STRSTARTS(STR(?o), \"{{BASE_IRI}}\")) } GROUP BY ?p ORDER BY DESC(?n) LIMIT 50",
        named_body: None,
        mixed_body: None,
        tier: Tier::Index,
        requires: &[Cap::BaseIri, Cap::ExternalIri],
        nonempty: NonEmpty::Witnessed,
        fallback: None,
        provably_empty: None,
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
        nonempty: NonEmpty::Witnessed,
        fallback: None,
        provably_empty: None,
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
        nonempty: NonEmpty::Witnessed,
        fallback: None,
        provably_empty: None,
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
        nonempty: NonEmpty::Aggregate,
        fallback: None,
        provably_empty: None,
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
        nonempty: NonEmpty::Aggregate,
        fallback: None,
        provably_empty: None,
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
        nonempty: NonEmpty::Witnessed,
        fallback: None,
        provably_empty: None,
    },
    // --- Space (WKT is always CRS84 lon/lat) ---
    Template {
        id: "sp-bbox",
        title: "Geographic extent",
        dimension: "space",
        question: "What is the bounding box of the wgs84 coordinates? (lon/lat)",
        // NOTE: the two predicates are conjoined on ONE subject, while the gate
        // (`geo_latlong`) is `lat seen` AND `long seen`, tallied independently —
        // the same shape as the `lb-labels` bug. It cannot produce zero rows
        // (an un-grouped aggregate always returns one), so the worst case is a
        // row of unbound values rather than a missing answer, and the card
        // cannot do better: `class_links` collapses every untyped subject into
        // one `(untyped)` bucket, so rows for `wgs:lat` and `wgs:long` under it
        // are not evidence that any single subject carries both.
        body: "SELECT (MIN(?lon) AS ?minLon) (MIN(?lat) AS ?minLat) (MAX(?lon) AS ?maxLon) (MAX(?lat) AS ?maxLat) WHERE { ?s wgs:long ?lon ; wgs:lat ?lat }",
        named_body: None,
        mixed_body: None,
        tier: Tier::Index,
        requires: &[Cap::GeoLatLong],
        nonempty: NonEmpty::Aggregate,
        fallback: None,
        provably_empty: None,
    },
    Template {
        id: "sp-within",
        title: "Features inside a box",
        dimension: "space",
        question: "Which features fall inside the dataset's bounding box? (WKT is lon/lat)",
        //
        // The access path is `{{WKT_PATH}}`, read off the predicates the card
        // recorded, because the fixed `geo:hasGeometry/geo:asWKT` was a third
        // instance of the same bug — and a live one. The gate, `signals.geo_wkt`,
        // is a three-way OR (`geo:asWKT` seen, OR `geo:hasGeometry` seen, OR a
        // `wktLiteral` datatype seen) while the body demanded BOTH steps of the
        // chain: `geoadmin` hangs `geo:asWKT` straight off each District and has
        // no `geo:hasGeometry` at all, so its published card ships a query that
        // cannot match — 0 rows on 52,959 geometries. `geo:hasGeometry?/geo:asWKT`
        // covers both layouts in one path (the optional step binds `?s` to the
        // feature when there is a geometry node, to the feature itself when the
        // WKT hangs off it directly).
        body: "SELECT ?s WHERE { ?s {{WKT_PATH}} ?w FILTER(geof:sfWithin(?w, \"{{BBOX_POLYGON}}\"^^geo:wktLiteral)) } LIMIT 100",
        named_body: None,
        mixed_body: None,
        tier: Tier::Index,
        requires: &[Cap::WktPath, Cap::Bbox],
        // The path is witnessed; the FILTER is not, and cannot be. The box is
        // derived from `wgs84:lat`/`wgs84:long` literals — a different signal
        // from the geometries this body reads, on possibly different subjects —
        // and a GeoSPARQL `wktLiteral` may carry a projected CRS whose
        // coordinates fall outside any lon/lat box, so not even the whole-world
        // fallback box is a proof. Emptiness here means "nothing in that box",
        // which is a real answer about the data, and the box is the parameter
        // the reader is meant to edit.
        nonempty: NonEmpty::Undecidable(
            "the box is derived from wgs84 lat/long literals, not from the WKT geometries this \
             query reads, and the card records no coordinate extent for them",
        ),
        fallback: None,
        provably_empty: None,
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
        nonempty: NonEmpty::Witnessed,
        fallback: None,
        provably_empty: None,
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
        nonempty: NonEmpty::Witnessed,
        fallback: None,
        provably_empty: None,
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
        nonempty: NonEmpty::Witnessed,
        fallback: None,
        provably_empty: None,
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
///
/// The *jointly derived* fields — [`Caps::labeled_class`], [`Caps::object_pred`],
/// [`Caps::wkt_path`], [`Caps::external_iri`] — are the point of this struct: each
/// is resolved from a `class_links` row (or a predicate list) that **witnesses**
/// the pattern its template will write, rather than from an independent ranking
/// that merely says the term exists somewhere in the graph.
struct Caps {
    top_class: Option<String>,
    top_pred: Option<String>,
    labeled_class: Option<String>,
    label_pred: Option<String>,
    object_pred: Option<String>,
    time_pred: Option<String>,
    num_pred: Option<String>,
    base_iri: Option<String>,
    external_iri: bool,
    wkt_path: Option<String>,
    bbox_polygon: Option<String>,
    geo_latlong: bool,
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

        // LABELED_CLASS — the most populous class the card can PROVE carries the
        // label predicate. `card.classes` is ordered by instance count, so the
        // first hit is the biggest labelled class, and it equals `top_class`
        // whenever the top class is labelled (the common case, which therefore
        // produces exactly the query it produced before). A `class_links` row is
        // a positive witness — some subject really has both statements — so a
        // hit is proof, and a miss (a truncated top-100 list, or labels only on
        // untyped subjects) is merely absence of proof, which is why the
        // template keeps a class-free fallback rather than vanishing.
        let label_pred = s.label_predicate.clone();
        let labeled_class = label_pred.as_ref().and_then(|lp| {
            card.classes
                .iter()
                .map(|(c, _)| c)
                .find(|c| {
                    card.class_links
                        .iter()
                        .any(|l| &&l.s_class == c && &l.predicate == lp)
                })
                .cloned()
        });

        // OBJECT_PRED — the most frequent relation the card can PROVE points at
        // something other than a literal, i.e. one a path query can walk.
        // `class_links` is ordered by count and excludes `rdf:type` rows.
        let object_pred = card
            .class_links
            .iter()
            .find(|l| l.o_class != O_LITERAL && l.predicate != RDF_TYPE)
            .map(|l| l.predicate.clone());

        // WKT_PATH — how this dataset actually hangs a geometry off a subject.
        // Read from the predicate list (capped at the top 100, so a miss can
        // only lose the query, never emit a broken one).
        let has_pred = |iri: &str| card.predicates.iter().any(|(p, _)| p == iri);
        let wkt_path = has_pred(GEO_ASWKT).then(|| {
            if has_pred(GEO_HASGEOMETRY) {
                // The optional step covers BOTH layouts at once: `?s` binds to
                // the feature when a geometry node sits in between, and to the
                // feature itself when the WKT hangs off it directly.
                "geo:hasGeometry?/geo:asWKT".to_string()
            } else {
                "geo:asWKT".to_string()
            }
        });
        let bbox_polygon = wkt_path.is_some().then(|| bbox_polygon(s.spatial_bbox));

        // EXTERNAL_IRI — an IRI the card records that does NOT start with the
        // dataset's base IRI, so the "links pointing out" filter keeps at least
        // one statement. Classes count: `?s a foaf:Person` is an IRI object like
        // any other. Blank nodes do not — the query filters on `isIRI`.
        let external_iri = s.base_iri.as_deref().is_some_and(|base| {
            card.classes
                .iter()
                .chain(card.in_hubs.iter())
                .any(|(term, _)| {
                    let iri = term.trim_start_matches('<').trim_end_matches('>');
                    !term.starts_with("_:") && !iri.starts_with(base)
                })
        });

        Caps {
            top_class: card.classes.first().map(|(c, _)| c.clone()),
            top_pred,
            labeled_class,
            label_pred,
            object_pred,
            time_pred: s.time_predicates.first().cloned(),
            num_pred: s.numeric_predicates.first().cloned(),
            base_iri: s.base_iri.clone(),
            external_iri,
            wkt_path,
            bbox_polygon,
            geo_latlong: s.geo_latlong,
            named_graphs: card.named_graph_count > 0,
            link: !s.link_predicates.is_empty(),
            has_literals: !card.datatypes.is_empty(),
        }
    }

    /// The text substituted for a capability's placeholder, if it has one.
    fn value(&self, cap: Cap) -> Option<&str> {
        match cap {
            Cap::TopClass => self.top_class.as_deref(),
            Cap::TopPred => self.top_pred.as_deref(),
            Cap::LabeledClass => self.labeled_class.as_deref(),
            Cap::LabelPred => self.label_pred.as_deref(),
            Cap::ObjectPred => self.object_pred.as_deref(),
            Cap::TimePred => self.time_pred.as_deref(),
            Cap::NumPred => self.num_pred.as_deref(),
            Cap::BaseIri => self.base_iri.as_deref(),
            Cap::WktPath => self.wkt_path.as_deref(),
            Cap::Bbox => self.bbox_polygon.as_deref(),
            Cap::ExternalIri
            | Cap::GeoLatLong
            | Cap::NamedGraphs
            | Cap::Link
            | Cap::HasLiterals => None,
        }
    }

    /// Is a capability available (string caps must resolve; bool caps must hold)?
    fn available(&self, cap: Cap) -> bool {
        match cap {
            Cap::ExternalIri => self.external_iri,
            Cap::GeoLatLong => self.geo_latlong,
            Cap::NamedGraphs => self.named_graphs,
            Cap::Link => self.link,
            Cap::HasLiterals => self.has_literals,
            other => self.value(other).is_some(),
        }
    }

    /// Substitute every placeholder we have a value for.
    fn substitute(&self, body: &str) -> String {
        let mut out = body.to_string();
        for &cap in ALL_CAPS {
            if let (Some(ph), Some(v)) = (cap.placeholder(), self.value(cap)) {
                out = out.replace(ph, v);
            }
        }
        out
    }
}

/// Audit one body against the co-occurrence rule (see the module docs).
///
/// Two things are checked, and both are properties of the **table**, not of any
/// dataset — so this runs over every template at test time and, in debug builds,
/// over every body the generator actually emits:
///
/// 1. Every `{{PLACEHOLDER}}` names a capability the template *requires*. A body
///    can therefore never be shipped with an unsubstituted hole, whatever the
///    card looks like.
/// 2. Unless the template declares itself [`NonEmpty::Undecidable`], any two
///    distinct capabilities appearing in the same body are [jointly
///    derived](Cap::joint_with). This is the rule `lb-labels` broke.
fn check_body(id: &str, body: &str, requires: &[Cap], nonempty: NonEmpty) -> Result<(), String> {
    let used: Vec<Cap> = ALL_CAPS
        .iter()
        .copied()
        .filter(|c| c.placeholder().is_some_and(|ph| body.contains(ph)))
        .collect();
    // (1) Placeholders are accounted for, in both directions.
    for cap in &used {
        if !requires.contains(cap) {
            return Err(format!(
                "{id}: body uses {} but does not require {}",
                cap.placeholder().unwrap_or_default(),
                cap.key()
            ));
        }
    }
    for chunk in body.split("{{").skip(1) {
        let name = chunk.split("}}").next().unwrap_or(chunk);
        let ph = format!("{{{{{name}}}}}");
        if !ALL_CAPS.iter().any(|c| c.placeholder() == Some(&ph[..])) {
            return Err(format!("{id}: body has an unknown placeholder {ph}"));
        }
    }
    // (2) Whatever meets in the body must have been chosen together.
    if matches!(nonempty, NonEmpty::Undecidable(_)) {
        return Ok(());
    }
    for (i, a) in used.iter().enumerate() {
        for b in &used[i + 1..] {
            if !a.joint_with().contains(b) {
                return Err(format!(
                    "{id}: conjoins {} with {}, which are chosen independently — \
                     make one jointly derived (Cap::joint_with) or declare NonEmpty::Undecidable",
                    a.key(),
                    b.key()
                ));
            }
        }
    }
    Ok(())
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
        // 1. Capability gate — the full template, or its reduced fallback. A
        //    fallback carries no scope variants, so it is default-graph shaped.
        let all = |rs: &[Cap]| rs.iter().all(|&c| caps.available(c));
        let (base, named, mixed, requires, nonempty) = if all(t.requires) {
            (t.body, t.named_body, t.mixed_body, t.requires, t.nonempty)
        } else if let Some(f) = t.fallback.as_ref().filter(|f| all(f.requires)) {
            (f.body, None, None, f.requires, f.nonempty)
        } else {
            continue;
        };

        // 2. Pick the body for where the data lives. A scope-variant body is
        //    always Index-tier: GRAPH/UNION patterns are not summary-shaped
        //    (`summary_query_shape` recognizes only a bare default-graph
        //    pattern), and the pyramid summary covers the default graph only.
        let (body, tier) = match scope {
            GraphScope::DefaultOnly => (base, t.tier),
            GraphScope::NamedOnly => match named {
                Some(b) => (b, Tier::Index),
                // GRAPH-native templates (the ng-* family) already address the
                // named graphs; any other default-graph body would be a
                // guaranteed-zero-rows query on a file whose default graph the
                // card itself records as empty — skip it.
                None if requires.contains(&Cap::NamedGraphs) => (base, t.tier),
                None => continue,
            },
            GraphScope::Mixed => match mixed {
                Some(b) => (b, Tier::Index),
                // The default-graph body is sound here — the profile that
                // instantiated it was derived from the (non-empty) default
                // graph — and the ng-* family surfaces the named half.
                None => (base, t.tier),
            },
        };

        // 3. The last gate: refuse a query the card can PROVE would answer
        //    nothing. Only in default-graph scope — the card's counts are over
        //    the default graph, so they witness nothing about a GRAPH-scoped
        //    body (and a `GRAPH`-native template is never asking about the
        //    default graph in the first place).
        if scope == GraphScope::DefaultOnly && t.provably_empty.is_some_and(|f| f(card)) {
            continue;
        }

        debug_assert!(
            check_body(t.id, body, requires, nonempty).is_ok(),
            "{}",
            check_body(t.id, body, requires, nonempty).unwrap_err()
        );
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
            requires: requires.iter().map(|c| c.key().to_string()).collect(),
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

    /// Ids whose template admits it cannot prove non-emptiness. The allow-list
    /// is **read off the table** rather than written out here, so a template
    /// cannot quietly join it: adding [`NonEmpty::Undecidable`] to one means
    /// writing down the reason, in the source, next to the body.
    fn may_be_empty(id: &str) -> bool {
        TEMPLATES
            .iter()
            .find(|t| t.id == id)
            .is_some_and(|t| matches!(t.nonempty, NonEmpty::Undecidable(_)))
    }

    /// The property that would have caught the named-graph-only card bug — and
    /// the `lb-labels` one: every query the card ships **returns a non-empty
    /// result** when run against the very graph it was generated for.
    ///
    /// The only exemptions are the templates that declare themselves
    /// undecidable; everything else answering nothing is a generator bug, not a
    /// fact about the dataset.
    fn assert_every_query_returns_rows(card: &DatasetCard, rete: &Rete) {
        assert!(!card.queries.is_empty(), "card generated no queries at all");
        for eq in &card.queries {
            let allow_empty = may_be_empty(&eq.id);
            match eval_query(rete, &eq.sparql) {
                Ok(QueryOutput::Select(_, rows)) => assert!(
                    allow_empty || !rows.is_empty(),
                    "{}: returned zero rows:\n{}",
                    eq.id,
                    eq.sparql
                ),
                Ok(QueryOutput::Ask(b)) => assert!(allow_empty || b, "{}: ASK false", eq.id),
                Ok(QueryOutput::Construct(ts)) => {
                    assert!(
                        allow_empty || !ts.is_empty(),
                        "{}: constructed nothing",
                        eq.id
                    )
                }
                // `QueryOutput` is non-exhaustive; the library only emits the
                // forms above.
                Ok(other) => panic!("{}: unexpected result form: {other:?}", eq.id),
                Err(e) => panic!("{}: failed to run: {e:?}", eq.id),
            }
        }
    }

    /// Build the fixture and return `(card, image)` — the pair every
    /// returns-rows test needs.
    fn card_and_graph(
        quads: Vec<(String, String, String, Option<String>)>,
        term_count: u64,
        named_graphs: u64,
    ) -> (DatasetCard, Rete) {
        let card = derive_card(&quads, term_count, named_graphs, CardInput::default());
        let (bytes, _) =
            ingest::assemble_dataset_with_opts(quads, true, false, None, |_, _| Vec::new());
        (card, Rete::open(&bytes).unwrap())
    }

    fn sparql_of<'a>(card: &'a DatasetCard, id: &str) -> Option<&'a str> {
        card.queries
            .iter()
            .find(|q| q.id == id)
            .map(|q| q.sparql.as_str())
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

        // The default-only scope gets the same returns-rows contract the
        // named-only and mixed ones have had since #154 — the gap through which
        // `lb-labels`, `top-reach` and `sp-within` shipped broken on
        // default-graph files, which is the population most datasets are in.
        assert_every_query_returns_rows(&card, &rete);
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

    /// The one-row smoke query returns EXACTLY one row in every graph scope —
    /// the unambiguous "did this file open and answer?" probe (a COUNT returns
    /// an honest 0 on a named-graph-only file and reads as failure).
    #[test]
    fn one_row_smoke_query_returns_exactly_one_row_in_every_scope() {
        type Quads = Vec<(String, String, String, Option<String>)>;
        let cases: Vec<(Quads, u64)> = vec![
            (rich_quads(), 0),       // default-only
            (named_only_quads(), 3), // named-graph-only
        ];
        for (quads, ng) in cases {
            let card = derive_card(&quads, 50, ng, CardInput::default());
            let smoke = card
                .queries
                .iter()
                .find(|q| q.id == "ov-one-row")
                .expect("ov-one-row always emits");
            let (bytes, _) =
                ingest::assemble_dataset_with_opts(quads, true, false, None, |_, _| Vec::new());
            let rete = Rete::open(&bytes).unwrap();
            match eval_query(&rete, &smoke.sparql).unwrap() {
                QueryOutput::Select(_, rows) => {
                    assert_eq!(rows.len(), 1, "ov-one-row must return exactly one row")
                }
                other => panic!("unexpected result form: {other:?}"),
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
            "ng-sample",
        ] {
            assert!(!ids.contains(&absent), "{absent} should be gated out");
        }
        // Unconditional overview queries are still present.
        for present in ["ov-triples", "ov-pred-list", "ov-pred-hist"] {
            assert!(ids.contains(&present), "{present} should always emit");
        }
        // Every object IRI is inside the dataset's own namespace, so "which
        // predicates link out?" has nothing to find — and is not emitted.
        assert!(
            !ids.contains(&"lk-external"),
            "lk-external has no external IRI"
        );
    }

    // ---------------------------------------------------------------------
    // The co-occurrence property: a body may conjoin only what the card proves
    // meets. Three fixtures, each reproducing the shape of a real dataset whose
    // published card ships a query that answers nothing.
    // ---------------------------------------------------------------------

    /// The **mtg** shape: the most populous class is NOT the one that carries
    /// the label predicate.
    ///
    /// `mtg:Ruling` has 76,990 instances and no `schema:name`; `mtg:Card` and
    /// its subclasses are fewer and carry all the names. A card generated from
    /// this graph must not join the top class to the label predicate, because
    /// the two never meet — that query returns zero rows on a plain
    /// default-graph file, and a reader concludes the file is broken.
    fn mtg_shaped_quads() -> Vec<(String, String, String, Option<String>)> {
        let name = "<http://schema.org/name>";
        let mut v = Vec::new();
        // 30 Rulings: the most populous class, no name anywhere on it.
        for i in 0..30 {
            let s = format!("<http://ex/ruling/{i}>");
            v.push(q(&s, TYPE, "<http://ex/Ruling>"));
            v.push(q(&s, "<http://ex/text>", &format!("\"Ruling text {i}\"")));
            v.push(q(
                &s,
                "<http://ex/about>",
                &format!("<http://ex/card/{}>", i % 5),
            ));
        }
        // 5 Cards: fewer instances, and the only things with a name.
        for i in 0..5 {
            let s = format!("<http://ex/card/{i}>");
            v.push(q(&s, TYPE, "<http://ex/Card>"));
            v.push(q(&s, name, &format!("\"Card {i}\"")));
            v.push(q(&s, "<http://ex/legalIn>", "<http://ex/format/standard>"));
        }
        v
    }

    #[test]
    fn top_class_without_the_label_predicate_still_answers() {
        let (card, rete) = card_and_graph(mtg_shaped_quads(), 60, 0);

        // The precondition of the bug, asserted so the fixture cannot drift
        // into the easy case: the top class is the one WITHOUT the label.
        assert_eq!(card.classes[0].0, "<http://ex/Ruling>");
        assert_eq!(
            card.signals.label_predicate.as_deref(),
            Some("<http://schema.org/name>")
        );
        assert!(
            !card
                .class_links
                .iter()
                .any(|l| l.s_class == "<http://ex/Ruling>"
                    && l.predicate == "<http://schema.org/name>"),
            "fixture precondition: the top class carries no label"
        );

        // The label query is still emitted — and it names the class that
        // actually has labels, not the biggest one.
        let lb = sparql_of(&card, "lb-labels").expect("lb-labels emitted");
        assert!(
            lb.contains("?s a <http://ex/Card> ; <http://schema.org/name> ?label"),
            "lb-labels must join the LABELLED class:\n{lb}"
        );
        assert!(
            !lb.contains("Ruling"),
            "the unlabelled top class leaked in:\n{lb}"
        );

        // Same class for the coverage query, so the two label queries are not
        // measuring different things (with TOP_CLASS this reported total/0).
        let cov = sparql_of(&card, "cmp-coverage").expect("cmp-coverage emitted");
        assert!(
            cov.contains("<http://ex/Card>"),
            "cmp-coverage on the wrong class"
        );

        // And the whole library answers.
        assert_every_query_returns_rows(&card, &rete);
    }

    /// The **hugging-face** shape: a label predicate exists, but no class the
    /// card can see carries it (there, `rdfs:label` sits only on the embedded
    /// ontology terms, whose `class_links` rows fall outside the top 100).
    /// The label query must degrade to the class-free form, not disappear.
    #[test]
    fn labels_off_every_known_class_fall_back_to_the_class_free_query() {
        let label = "<http://www.w3.org/2000/01/rdf-schema#label>";
        let mut quads = Vec::new();
        for i in 0..8 {
            let s = format!("<http://ex/model/{i}>");
            quads.push(q(&s, TYPE, "<http://ex/Model>"));
            quads.push(q(&s, "<http://ex/downloads>", &format!("\"{i}\"")));
        }
        // The only labels in the graph hang off UNTYPED subjects, so no
        // `class_links` row can witness a class that carries the predicate.
        for i in 0..3 {
            quads.push(q(
                &format!("<http://ex/term/{i}>"),
                label,
                &format!("\"Term {i}\"@en"),
            ));
        }
        let (card, rete) = card_and_graph(quads, 30, 0);

        assert_eq!(card.signals.label_predicate.as_deref(), Some(label));
        let lb = sparql_of(&card, "lb-labels").expect("lb-labels still emitted");
        assert!(
            lb.ends_with("SELECT ?s ?label WHERE { ?s <http://www.w3.org/2000/01/rdf-schema#label> ?label } LIMIT 50"),
            "expected the class-free fallback:\n{lb}"
        );
        // The fallback advertises the reduced gate it was emitted under.
        let req = &card
            .queries
            .iter()
            .find(|x| x.id == "lb-labels")
            .unwrap()
            .requires;
        assert_eq!(req, &["LABEL_PRED".to_string()]);
        // Coverage needs a labelled class and honestly has none, so it is gone
        // rather than reporting a percentage of the wrong class.
        assert!(sparql_of(&card, "cmp-coverage").is_none());
        assert_every_query_returns_rows(&card, &rete);
    }

    /// The **geoadmin** shape: `geo:asWKT` hangs straight off each feature and
    /// there is no `geo:hasGeometry` anywhere, so the fixed two-step path the
    /// template used to write could not match a single one of its 52,959
    /// geometries.
    #[test]
    fn geometry_hung_directly_off_the_feature_is_reachable() {
        let aswkt = "<http://www.opengis.net/ont/geosparql#asWKT>";
        let wkt = "http://www.opengis.net/ont/geosparql#wktLiteral";
        let mut quads = Vec::new();
        for i in 0..5 {
            let s = format!("<http://ex/district/{i}>");
            quads.push(q(&s, TYPE, "<http://ex/District>"));
            quads.push(q(&s, aswkt, &format!("\"POINT({} 4{i})\"^^<{wkt}>", i + 7)));
        }
        let (card, rete) = card_and_graph(quads, 20, 0);

        let sp = sparql_of(&card, "sp-within").expect("sp-within emitted");
        assert!(
            sp.contains("?s geo:asWKT ?w"),
            "expected the direct geometry path:\n{sp}"
        );
        assert!(
            !sp.contains("hasGeometry"),
            "a hasGeometry step the data does not have:\n{sp}"
        );
        match eval_query(&rete, sp).unwrap() {
            QueryOutput::Select(_, rows) => {
                assert_eq!(rows.len(), 5, "every feature is in the box")
            }
            other => panic!("unexpected result form: {other:?}"),
        }
        assert_every_query_returns_rows(&card, &rete);
    }

    /// Both geometry layouts in one graph: the optional step binds `?s` to the
    /// feature either way, so neither half is hidden.
    #[test]
    fn both_geometry_layouts_are_covered_by_one_path() {
        let (card, rete) = card_and_graph(rich_quads(), 50, 0);
        let sp = sparql_of(&card, "sp-within").expect("sp-within emitted");
        assert!(
            sp.contains("?s geo:hasGeometry?/geo:asWKT ?w"),
            "expected the optional-step path:\n{sp}"
        );
        match eval_query(&rete, sp).unwrap() {
            QueryOutput::Select(_, rows) => assert!(!rows.is_empty()),
            other => panic!("unexpected result form: {other:?}"),
        }
    }

    /// A graph of pure values — every object a literal, no `rdf:type` — has no
    /// group for the in-degree query and no IRI for the dangling-IRI one. The
    /// card knows (`in_hubs` is that precompute), so the generator refuses to
    /// emit either rather than shipping two queries that answer nothing.
    #[test]
    fn a_literal_only_graph_gets_no_object_queries() {
        let quads: Vec<_> = (0..6)
            .map(|i| {
                q(
                    &format!("<http://ex/row/{i}>"),
                    "<http://ex/value>",
                    &format!("\"v{i}\""),
                )
            })
            .collect();
        let (card, rete) = card_and_graph(quads, 12, 0);

        assert!(
            card.in_hubs.is_empty(),
            "fixture precondition: no IRI objects"
        );
        let ids: Vec<&str> = card.queries.iter().map(|x| x.id.as_str()).collect();
        for absent in ["top-in-hubs", "top-dangling"] {
            assert!(!ids.contains(&absent), "{absent} would return zero rows");
        }
        assert_every_query_returns_rows(&card, &rete);
    }

    /// The table-level audit: nothing in the library may conjoin two
    /// independently-chosen substitutions, and every placeholder must be
    /// declared. This is the check that makes the bug unrepresentable rather
    /// than merely fixed — it runs over every body of every template, in every
    /// graph scope, with no dataset involved.
    #[test]
    fn every_template_body_is_witnessed() {
        let mut problems = Vec::new();
        for t in TEMPLATES {
            for body in [Some(t.body), t.named_body, t.mixed_body]
                .into_iter()
                .flatten()
            {
                if let Err(e) = check_body(t.id, body, t.requires, t.nonempty) {
                    problems.push(e);
                }
            }
            if let Some(f) = &t.fallback {
                if let Err(e) = check_body(t.id, f.body, f.requires, f.nonempty) {
                    problems.push(e);
                }
                assert!(
                    f.requires.len() < t.requires.len(),
                    "{}: a fallback must need LESS than the template it stands in for",
                    t.id
                );
            }
        }
        assert!(problems.is_empty(), "{}", problems.join("\n"));
    }

    /// Ids are unique and every capability the table names is reachable — a
    /// `Cap` no template requires is dead weight that will drift out of date.
    #[test]
    fn the_table_is_well_formed() {
        let mut ids: Vec<&str> = TEMPLATES.iter().map(|t| t.id).collect();
        let n = ids.len();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), n, "duplicate template id");
        for &cap in ALL_CAPS {
            assert!(
                TEMPLATES.iter().any(|t| t.requires.contains(&cap)
                    || t.fallback
                        .as_ref()
                        .is_some_and(|f| f.requires.contains(&cap))),
                "{} is required by no template",
                cap.key()
            );
        }
    }
}
