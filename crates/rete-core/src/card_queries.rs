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
//! Bodies are **graph-scope aware** (`GraphScope`): a dataset whose statements
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
//! `NonEmpty` claim — the reason its emitted form cannot come back empty —
//! and `check_body` enforces the one rule that claim rests on:
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
//! must be **jointly derived** (`Cap::joint_with`) — chosen *because* they
//! meet, with a `class_links` row as the witness.
//!
//! Where the card genuinely cannot decide emptiness, the template says so
//! (`NonEmpty::Undecidable`) instead of pretending; `provably_empty` is the
//! last gate, dropping a template the card can prove would answer nothing.
//!
//! This is pure serde generation — no format change, no I/O, no threads. It
//! lives in `rete-core` (it used to live in the binary-only `rete-cli`, where no
//! client could reach it — #152) so the CLI, the browser builder and every
//! language binding instantiate one query library from one table.

use serde::Serialize;

use crate::card_derive::{
    DatasetCard, ExampleQuery, Tier, CARD_TOP_N, GEO_ASWKT, GEO_HASGEOMETRY, O_LITERAL,
};
use crate::RDF_TYPE;

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
    /// see the module docs. The relation is symmetric — `check_body` checks
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

/// **The** co-occurrence witness: does the card record that some subject of
/// `class` also carries `pred`?
///
/// A `class_links` row is positive evidence — the quotient counts real
/// statements, so a row exists only because a subject classified as `class`
/// really made a `pred` statement. Both the generator (choosing
/// `Cap::LabeledClass`) and the audit ([`audit`]) ask this one question, so
/// "does the card prove these meet?" has exactly one implementation.
fn class_carries(card: &DatasetCard, class: &str, pred: &str) -> bool {
    card.class_links
        .iter()
        .any(|l| l.s_class == class && l.predicate == pred)
}

/// Why a template's emitted query cannot come back empty. Declared per template
/// and enforced by `check_body` plus the fixture tests, which run every
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
    /// Is `Template::provably_empty` an **equivalence** in default-graph
    /// scope — does `false` prove the query answers, and not merely fail to
    /// prove it empty?
    ///
    /// True only where the card's precompute *is* this query's result:
    /// `top-in-hubs` is `card.in_hubs`, row for row and by construction (see
    /// the in-degree tally in `derive_card_from`). `top-dangling` shares the
    /// hook — no IRI object means nothing can dangle — without the converse, so
    /// it stays honestly undecidable. Nothing in generation reads this; it is
    /// what lets [`audit`] tell a query the card decides from one it does not,
    /// instead of counting both as unknown.
    hook_is_exact: bool,
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
        hook_is_exact: false,
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
        hook_is_exact: false,
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
        hook_is_exact: false,
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
        hook_is_exact: false,
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
        hook_is_exact: false,
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
        hook_is_exact: false,
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
        hook_is_exact: false,
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
        hook_is_exact: false,
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
        hook_is_exact: false,
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
        hook_is_exact: false,
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
        hook_is_exact: false,
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
        hook_is_exact: false,
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
        // `in_hubs` is this query's own precompute, so the hook decides it in
        // both directions once the body is default-graph scoped.
        hook_is_exact: true,
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
        hook_is_exact: false,
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
        hook_is_exact: false,
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
        hook_is_exact: false,
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
        hook_is_exact: false,
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
        hook_is_exact: false,
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
        hook_is_exact: false,
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
        hook_is_exact: false,
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
        hook_is_exact: false,
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
        hook_is_exact: false,
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
        hook_is_exact: false,
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
        hook_is_exact: false,
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
        hook_is_exact: false,
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
        hook_is_exact: false,
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
        hook_is_exact: false,
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
        hook_is_exact: false,
    },
];

/// What the template behind an emitted query claims about its emptiness — the
/// `NonEmpty` declaration, read back by id.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Claim {
    /// The template asserts the emitted query cannot come back empty
    /// (`NonEmpty::AnyGraph` / `NonEmpty::Aggregate` / `NonEmpty::Witnessed`).
    CannotBeEmpty,
    /// The template admits the card cannot decide it
    /// (`NonEmpty::Undecidable`).
    Undecidable,
    /// No template of this revision owns the id — a curated query, or one an
    /// older revision wrote. It claims nothing, so nothing is contradicted.
    Unknown,
}

/// The [`Claim`] behind a shipped query id.
///
/// The build measures every starter query against the finished file
/// (`commands::buildinfo`), which makes emptiness a **measured** fact rather
/// than a derived one — and where measurement and this claim disagree, the
/// claim is wrong. A query the table swears cannot be empty and that then
/// answers nothing is a defect in the table, not a fact about the dataset, and
/// `commands::build` says so out loud and records it. `Undecidable` is the
/// opposite: the template said in advance it could not know, so a measured zero
/// is expected news, not a contradiction.
pub fn claim_of(id: &str) -> Claim {
    match TEMPLATES.iter().find(|t| t.id == id) {
        Some(t) if matches!(t.nonempty, NonEmpty::Undecidable(_)) => Claim::Undecidable,
        Some(_) => Claim::CannotBeEmpty,
        None => Claim::Unknown,
    }
}

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
                .find(|c| class_carries(card, c, lp))
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
/// 2. Unless the template declares itself `NonEmpty::Undecidable`, any two
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
/// lives (`GraphScope`), placeholders substituted, and the shared PREFIX block
/// prepended.
pub fn generate(card: &DatasetCard) -> Vec<ExampleQuery> {
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

// ===========================================================================
// Auditing a card that is ALREADY published
// ===========================================================================
//
// [`generate`] decides what a card *should* ship. The audit answers the other
// half of the same question — what does a card that is already on the wire
// ship, and can it answer? — for a file whose queries were written by an older
// revision of the table above.
//
// It is deliberately in this module and not beside it: the one judgement it
// makes ("does the card prove these two pieces meet?") is
// `class_carries`, the same function `Caps::from_card` resolves
// `Cap::LabeledClass` with, and the emptiness gates it applies are the
// templates' own `Template::provably_empty` hooks and `NonEmpty` claims. A
// second, drifting copy of "is this empty" living in a script is how the
// original defect survived four releases.

/// What a published starter query is worth on the file that carries it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Verdict {
    /// The card **proves** at least one row comes back.
    Answers,
    /// The card **proves** none does. Airtight for the shapes that turn on a
    /// term's existence (a path step missing from a complete predicate list, a
    /// `VALUES` list disjoint from the link predicates, a default-graph body on
    /// an empty default graph); for the class-∧-predicate shape it rests on the
    /// quotient, whose one blind spot `Refuted` documents and narrows.
    Empty,
    /// A row comes back (an un-grouped aggregate always returns one) but it is
    /// vacuous — the thing being counted is provably zero.
    Vacuous,
    /// The card cannot prove the join is non-empty, and bounds how much could
    /// survive it. Not the same as `Empty`: reported separately, never merged.
    Suspect,
    /// Honestly undecidable from a card — the template says so, or the query
    /// conjoins terms nothing in the card can relate.
    Undecidable,
    /// The body matches no known revision of its template, so nothing is
    /// claimed about it.
    Unrecognized,
}

impl Verdict {
    pub fn as_str(self) -> &'static str {
        match self {
            Verdict::Answers => "answers",
            Verdict::Empty => "empty",
            Verdict::Vacuous => "vacuous",
            Verdict::Suspect => "suspect",
            Verdict::Undecidable => "undecidable",
            Verdict::Unrecognized => "unrecognized",
        }
    }
}

/// One published query's verdict, with the evidence behind it.
#[derive(Debug, Clone, Serialize)]
pub struct Finding {
    pub id: String,
    pub verdict: Verdict,
    /// The evidence, in the words a maintainer needs to act on it.
    pub why: String,
    /// What a re-card would do to this query: `current` (byte-identical to what
    /// this generator writes for this very card — nothing to fix),
    /// `superseded` (the generator writes a different body now), or `dropped`
    /// (it would no longer be emitted at all).
    pub revision: &'static str,
    /// The terms substituted into the body, each tagged with the card ranking
    /// it was drawn from. Two terms from **different** rankings conjoined in
    /// one body is the #172 shape — recorded for every query, so the census is
    /// empirical rather than a re-reading of the table.
    pub substitutions: Vec<Substitution>,
    /// True when the body conjoins two or more independently-ranked terms.
    pub conjoined: bool,
    /// What running the query actually did — absent unless `--measure` ran it.
    /// Kept **beside** `verdict`, never merged into it: one is what a card can
    /// prove about a file, the other is what the file did. Where they disagree
    /// the observation wins, and the disagreement is the finding.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub observed: Option<Observation>,
}

/// One starter query, actually run.
#[derive(Debug, Clone, Serialize)]
pub struct Observation {
    /// `answers` (rows with bindings), `vacuous` (rows that bind nothing),
    /// `empty` (no rows at all), or `error` (the run did not finish).
    pub outcome: &'static str,
    pub rows: u64,
    /// Bytes the run read, cold, open included.
    pub bytes: u64,
    /// Range requests the run made, cold, open included.
    pub requests: u64,
    /// Wall clock on the measuring machine — a reference, not a property of the
    /// file. Read it next to `bytes`/`requests`, never on its own.
    pub debug_ms: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// What the file's own build record says this query cost — present only
    /// when the file carries one. That is the single case where a
    /// re-measurement has a **known answer to check itself against**, so it is
    /// checked, and the answer is reported rather than assumed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub recorded: Option<Recorded>,
}

/// The figures a build wrote into the file, beside the ones just measured.
#[derive(Debug, Clone, Serialize)]
pub struct Recorded {
    pub bytes: u64,
    pub requests: u64,
    pub rows: u64,
    /// All three agree with the run. `bytes` and `requests` are properties of
    /// layout + query, so they should — through the same transport. A
    /// disagreement is worth knowing about: a different reader fan-out, a
    /// different engine, or a file that is not the one the record describes.
    pub agrees: bool,
}

impl Observation {
    /// Does the observation contradict what the card alone concluded? Only
    /// `Answers` and `Empty` are claims strong enough to be wrong; `Suspect`
    /// and `Undecidable` say in so many words that they are not claims.
    pub fn contradicts(&self, verdict: Verdict) -> bool {
        match verdict {
            Verdict::Answers => self.outcome != "answers",
            Verdict::Empty => self.outcome == "answers",
            Verdict::Vacuous => self.outcome == "answers",
            _ => false,
        }
    }
}

/// A substituted term and where in the card profile it came from.
#[derive(Debug, Clone, Serialize)]
pub struct Substitution {
    pub placeholder: String,
    pub value: String,
    /// e.g. `classes[0]`, `predicates[3]`, `signals.label_predicate`. Empty
    /// when the term is in no ranking the card records (a hard-coded IRI, or a
    /// value that fell off a capped list).
    pub origin: Vec<String>,
}

/// Bodies that **earlier revisions** of a template shipped. A published file is
/// not re-cardable for free, so the audit has to read the query text that is
/// actually out there; only the revisions the current bodies no longer match
/// need an entry. Every one is a body `check_body` would reject today — that
/// is why it has an entry.
struct Legacy {
    id: &'static str,
    body: &'static str,
}

const LEGACY_BODIES: &[Legacy] = &[
    // pre-#172 `top-reach`: the busiest subject and the most frequent
    // predicate, chosen from two independent rankings and then conjoined.
    Legacy {
        id: "top-reach",
        body: "SELECT ?y WHERE { {{HUB_IRI}} {{TOP_PRED}}+ ?y } LIMIT 100",
    },
    // pre-#172 `lk-sameas`: the VALUES list covered three of the four
    // predicates the `signals.link_predicates` gate is detected from.
    Legacy {
        id: "lk-sameas",
        body: "SELECT ?p (COUNT(*) AS ?n) WHERE { ?s ?p ?o VALUES ?p { owl:sameAs skos:exactMatch rdfs:seeAlso } } GROUP BY ?p",
    },
];

/// The effective top-N cap a card's profile lists were derived under. Cards
/// built before the field existed omit it; they were built by this same builder
/// family, whose cap is a compile-time constant.
fn cap_of(card: &DatasetCard) -> usize {
    if card.top_n > 0 {
        card.top_n as usize
    } else {
        CARD_TOP_N
    }
}

/// Is a capped profile list **complete** — did nothing fall off the end? A list
/// shorter than the cap was never truncated, so absence from it is evidence of
/// absence. A list *at* the cap proves nothing either way.
fn complete<T>(card: &DatasetCard, list: &[T]) -> bool {
    list.len() < cap_of(card)
}

/// How far the card can go towards proving that no instance of `class` carries
/// `pred` — the negative of `class_carries`, which a card can rarely settle
/// outright.
/// The refutation is exact **for singly-typed data only**, and that is the
/// audit's one standing caveat: the quotient gives every subject exactly one
/// class (the last `rdf:type` wins), so a subject typed both `class` and
/// `other` is counted under `other` alone. `?s a class ; pred ?o` then matches a
/// graph whose quotient shows no such row — `vidy` types all 39,723 of its units
/// both `schema:ArchiveComponent` and `vidy:Unit`, the quotient files every one
/// of them under `vidy:Unit`, and the label query the card ships returns 50 rows
/// regardless.
///
/// A card cannot record multi-typing, but it does leave two tells, and
/// [`refute_class_pred`] refuses to claim a refutation when either shows:
///
///  1. **The class is absent from the quotient.** A class with instances but no
///     `class_links` row of its own is a class the quotient is not describing —
///     shadowed, or making no statements at all — so its silence about one
///     predicate means nothing.
///  2. **Another class has exactly the same instance count.** Two classes over
///     the same population is what dual typing looks like from the card (a
///     schema.org type beside a native one — `vidy` and `arxiu` both), and the
///     labelled half may be filed under the twin.
///
/// Neither is a proof of single-typing; together they are what the card can see.
/// Running the query is what closes it.
enum Refuted {
    /// Every use of `pred` is accounted for in the `class_links` quotient, none
    /// of them falls on `class`, and neither multi-typing tell shows.
    Proved,
    /// The quotient is truncated: at most `budget` uses of `pred` are
    /// unaccounted for, so at most that many of the class's `instances` could
    /// be joined. Suspicion, not proof.
    AtMost { budget: u64, instances: u64 },
    /// The quotient accounts for every use of `pred` and none falls on `class`,
    /// but a multi-typing tell shows, so that silence proves nothing.
    Shadowed { described: bool, twin: bool },
    /// `pred` is not in the (capped) predicate list, so there is no total to
    /// account against.
    Unknown,
}

fn refute_class_pred(card: &DatasetCard, class: &str, pred: &str) -> Refuted {
    let instances = card
        .classes
        .iter()
        .find(|(c, _)| c == class)
        .map(|(_, n)| *n)
        .unwrap_or(0);
    // Every non-`rdf:type` statement contributes exactly one `class_links` row,
    // so the rows for a predicate sum to its total use **iff** none was
    // truncated away — the completeness test that makes absence conclusive.
    let accounted: u64 = card
        .class_links
        .iter()
        .filter(|l| l.predicate == pred)
        .map(|l| l.count)
        .sum();
    let Some((_, total)) = card.predicates.iter().find(|(p, _)| p == pred) else {
        return Refuted::Unknown;
    };
    let accounted_for_all = accounted >= *total || complete(card, &card.class_links);
    if !accounted_for_all {
        return Refuted::AtMost {
            budget: total - accounted,
            instances,
        };
    }
    // The two multi-typing tells (see `Refuted`). Either one and the quotient's
    // silence about this class stops being evidence.
    let described = card.class_links.iter().any(|l| l.s_class == class);
    let twin = card
        .classes
        .iter()
        .any(|(c, n)| c != class && *n == instances);
    if described && !twin {
        Refuted::Proved
    } else {
        Refuted::Shadowed { described, twin }
    }
}

/// Which card rankings a substituted term appears in. Read off the card rather
/// than declared by the template, so it also describes a body this table no
/// longer contains.
fn origins(card: &DatasetCard, v: &str) -> Vec<String> {
    let mut o = Vec::new();
    if let Some(i) = card.classes.iter().position(|(c, _)| c == v) {
        o.push(format!("classes[{i}]"));
    }
    if let Some(i) = card.predicates.iter().position(|(p, _)| p == v) {
        o.push(format!("predicates[{i}]"));
    }
    if let Some(i) = card.top_hubs.iter().position(|(h, _)| h == v) {
        o.push(format!("top_hubs[{i}]"));
    }
    let s = &card.signals;
    if s.label_predicate.as_deref() == Some(v) {
        o.push("signals.label_predicate".to_string());
    }
    if let Some(i) = s.time_predicates.iter().position(|p| p == v) {
        o.push(format!("signals.time_predicates[{i}]"));
    }
    if let Some(i) = s.numeric_predicates.iter().position(|p| p == v) {
        o.push(format!("signals.numeric_predicates[{i}]"));
    }
    if s.base_iri.as_deref() == Some(v) {
        o.push("signals.base_iri".to_string());
    }
    o
}

/// The bindings a published body gave a pattern's placeholders.
type Binds = Vec<(String, String)>;

/// The **distinct** placeholders a body substitutes with a term the card
/// ranks. Distinct by name on purpose: the current `top-reach` writes
/// `{{OBJECT_PRED}}` twice — once to pick the seed, once to walk from it — and
/// a term conjoined with itself is the opposite of the two-maxima shape, it is
/// how the template guarantees the seed has an edge.
fn distinct_ranked<'a>(card: &DatasetCard, binds: &'a Binds) -> Vec<&'a str> {
    let mut names: Vec<&str> = binds
        .iter()
        .filter(|(_, v)| !origins(card, v).is_empty())
        .map(|(n, _)| n.as_str())
        .collect();
    names.sort_unstable();
    names.dedup();
    names
}

fn bound<'a>(binds: &'a Binds, name: &str) -> Option<&'a str> {
    binds
        .iter()
        .find(|(n, _)| n == name)
        .map(|(_, v)| v.as_str())
}

/// Match a published body against a template pattern, recovering the value each
/// `{{PLACEHOLDER}}` took. Every literal segment must appear, in order, and the
/// body must end exactly where the pattern does — a match is exact, so a
/// mismatched revision is reported as such instead of being misread.
fn match_pattern(pattern: &str, body: &str) -> Option<Binds> {
    let mut binds: Binds = Vec::new();
    let mut pat = pattern;
    let mut rest = body;
    loop {
        let Some(i) = pat.find("{{") else {
            return (pat == rest).then_some(binds);
        };
        let (lit, tail) = pat.split_at(i);
        rest = rest.strip_prefix(lit)?;
        let end = tail.find("}}")?;
        let name = &tail[2..end];
        pat = &tail[end + 2..];
        // The value runs up to the pattern's next literal segment.
        let next_lit = &pat[..pat.find("{{").unwrap_or(pat.len())];
        let cut = if next_lit.is_empty() {
            rest.len()
        } else {
            rest.find(next_lit)?
        };
        if cut == 0 {
            return None; // an empty substitution is not a substitution
        }
        binds.push((name.to_string(), rest[..cut].to_string()));
        rest = &rest[cut..];
    }
}

/// The `?s a {{CLASS}} … {{PRED}}` shape — a class and a predicate conjoined on
/// **one** subject. Recognized from the pattern text, not from the template id,
/// so a future template that writes the same shape is audited too.
fn class_pred_conjunction(pattern: &str) -> Option<(&str, &str)> {
    let after = pattern.split("?s a {{").nth(1)?;
    let class = after.split("}}").next()?;
    let tail = after.split_once("}}")?.1;
    for sep in ["; {{", "?s {{"] {
        if let Some(x) = tail.split(sep).nth(1) {
            return Some((class, x.split("}}").next()?));
        }
    }
    None
}

/// The `VALUES ?p { … }` list a body pins itself to, as bracketed IRIs. The
/// gate that admits the query counts predicates; the body only matches the ones
/// it names, and the two can disagree.
fn values_predicates(body: &str) -> Option<Vec<String>> {
    let inner = body.split("VALUES ?p {").nth(1)?.split('}').next()?;
    Some(
        inner
            .split_whitespace()
            .filter_map(|t| {
                let (p, local) = t.split_once(':')?;
                let ns = match p {
                    "owl" => "http://www.w3.org/2002/07/owl#",
                    "skos" => "http://www.w3.org/2004/02/skos/core#",
                    "rdfs" => "http://www.w3.org/2000/01/rdf-schema#",
                    _ => return None,
                };
                Some(format!("<{ns}{local}>"))
            })
            .collect(),
    )
}

/// The predicate IRIs a property path **must** traverse — the steps that are
/// not optional. `geo:hasGeometry?/geo:asWKT` requires only the second; the
/// fixed `geo:hasGeometry/geo:asWKT` the pre-#172 template wrote requires both,
/// which is why it could not match a file that has no `geo:hasGeometry`.
fn required_path_steps(path: &str) -> Vec<String> {
    path.split('/')
        .filter(|step| !step.ends_with('?') && !step.ends_with('*'))
        .filter_map(|step| match step.trim_end_matches('+') {
            "geo:asWKT" => Some(GEO_ASWKT.to_string()),
            "geo:hasGeometry" => Some(GEO_HASGEOMETRY.to_string()),
            _ => None,
        })
        .collect()
}

/// Audit every starter query a card already ships.
///
/// Two things happen per query, and they answer different questions. [`generate`]
/// is re-run over the same profile — what a re-card would ship — which says
/// whether the query is *stale*. The verdict says whether it *answers*, which is
/// the thing a reader of the published file experiences today.
pub fn audit(card: &DatasetCard) -> Vec<Finding> {
    let caps = Caps::from_card(card);
    let scope = GraphScope::of(card);
    let fresh = generate(card);
    card.queries
        .iter()
        .map(|q| {
            let revision = match fresh.iter().find(|r| r.id == q.id) {
                Some(r) if r.sparql == q.sparql => "current",
                Some(_) => "superseded",
                None => "dropped",
            };
            finding(card, &caps, scope, q, revision)
        })
        .collect()
}

fn finding(
    card: &DatasetCard,
    caps: &Caps,
    scope: GraphScope,
    q: &ExampleQuery,
    revision: &'static str,
) -> Finding {
    let body = q
        .sparql
        .lines()
        .filter(|l| !l.starts_with("PREFIX"))
        .collect::<Vec<_>>()
        .join("\n");
    let body = body.trim();

    let mk = |verdict, why: String, revision, binds: &Binds| {
        let substitutions: Vec<Substitution> = binds
            .iter()
            .map(|(n, v)| Substitution {
                placeholder: n.clone(),
                value: v.clone(),
                origin: origins(card, v),
            })
            .collect();
        Finding {
            id: q.id.clone(),
            verdict,
            why,
            revision,
            conjoined: distinct_ranked(card, binds).len() > 1,
            substitutions,
            observed: None,
        }
    };

    let Some(t) = TEMPLATES.iter().find(|t| t.id == q.id) else {
        return mk(
            Verdict::Unrecognized,
            "no template of this id is in the library any more".to_string(),
            "retired",
            &Vec::new(),
        );
    };

    // Recover the substituted terms: match the body against the template's own
    // patterns (every scope variant and the fallback), then against the bodies
    // earlier revisions wrote. The *values* decide the verdict, so a pattern
    // shared by two revisions is not a problem — `revision` already says which
    // one a re-card would replace it with.
    let patterns = [
        Some(t.body),
        t.named_body,
        t.mixed_body,
        t.fallback.as_ref().map(|f| f.body),
    ]
    .into_iter()
    .flatten()
    .chain(
        LEGACY_BODIES
            .iter()
            .filter(|l| l.id == q.id)
            .map(|l| l.body),
    );
    let Some((pattern, binds)) = patterns.into_iter().find_map(|p| {
        // Longest pattern first would be nicer, but the bodies of one template
        // are not prefixes of each other — an exact match is unambiguous.
        match_pattern(p, body).map(|b| (p, b))
    }) else {
        return mk(
            Verdict::Unrecognized,
            "the body matches no known revision of this template".to_string(),
            "unknown",
            &Vec::new(),
        );
    };

    // --- 1. Scope. A default-graph body on a file whose default graph the card
    //        itself records as empty cannot return a row (issue #170).
    if !body.contains("GRAPH") && card.triple_count == 0 && card.named_graph_count > 0 {
        return mk(
            Verdict::Empty,
            format!(
                "the body scans the default graph, which the card records as empty \
                 ({} named graph(s) hold all {} statements)",
                card.named_graph_count, card.quad_count
            ),
            revision,
            &binds,
        );
    }

    // --- 2. The template's own last gate, unchanged — and, where the table says
    //        the hook is an equivalence, its other direction.
    if scope == GraphScope::DefaultOnly {
        if let Some(hook) = t.provably_empty {
            if hook(card) {
                return mk(
                    Verdict::Empty,
                    "the card's own precompute for this query is empty (the generator \
                     refuses to emit it today)"
                        .to_string(),
                    revision,
                    &binds,
                );
            }
            if t.hook_is_exact {
                return mk(
                    Verdict::Answers,
                    "the card precomputes this query's own result, and it is not empty".to_string(),
                    revision,
                    &binds,
                );
            }
        }
    }

    // --- 3. The co-occurrence rule, applied to the body that shipped.
    if let Some((cn, pn)) = class_pred_conjunction(pattern) {
        if let (Some(class), Some(pred)) = (bound(&binds, cn), bound(&binds, pn)) {
            let aggregate = matches!(t.nonempty, NonEmpty::Aggregate);
            if class_carries(card, class, pred) {
                return mk(
                    Verdict::Answers,
                    format!("class_links witnesses {class} carrying {pred}"),
                    revision,
                    &binds,
                );
            }
            return match refute_class_pred(card, class, pred) {
                Refuted::Proved => mk(
                    if aggregate {
                        Verdict::Vacuous
                    } else {
                        Verdict::Empty
                    },
                    format!(
                        "the class_links quotient accounts for every use of {pred} and \
                         none falls on {class}{}",
                        if aggregate {
                            " — the aggregate returns one row whose count is 0"
                        } else {
                            ""
                        }
                    ),
                    revision,
                    &binds,
                ),
                Refuted::AtMost { budget, instances } => mk(
                    Verdict::Suspect,
                    format!(
                        "no class_links row witnesses {class} carrying {pred}; the quotient \
                         is truncated, so at most {budget} of the class's {instances} \
                         instances could be joined"
                    ),
                    revision,
                    &binds,
                ),
                Refuted::Shadowed { described, twin } => mk(
                    Verdict::Suspect,
                    format!(
                        "the quotient accounts for every use of {pred} and none falls on \
                         {class}, but {} — the quotient files a multi-typed subject under \
                         one class only, so its silence is not evidence",
                        match (described, twin) {
                            (false, true) =>
                                "the class has no class_links row of its own AND another class \
                                 has exactly its instance count",
                            (false, false) => "the class has no class_links row of its own",
                            _ => "another class has exactly its instance count",
                        }
                    ),
                    revision,
                    &binds,
                ),
                Refuted::Unknown => mk(
                    Verdict::Undecidable,
                    format!("{pred} is not in the card's (capped) predicate list"),
                    revision,
                    &binds,
                ),
            };
        }
    }

    // --- 4. A property path can only match if the steps it insists on exist.
    if let Some(path) = bound(&binds, "WKT_PATH") {
        let missing: Vec<String> = required_path_steps(path)
            .into_iter()
            .filter(|iri| !card.predicates.iter().any(|(p, _)| p == iri))
            .collect();
        if !missing.is_empty() && complete(card, &card.predicates) {
            return mk(
                Verdict::Empty,
                format!(
                    "the path {path} requires {} , which the card's complete predicate \
                     list does not contain",
                    missing.join(", ")
                ),
                revision,
                &binds,
            );
        }
    }

    // --- 5. A pinned VALUES list must cover the gate that admitted the query.
    if let Some(listed) = values_predicates(body) {
        let present: Vec<&String> = card
            .signals
            .link_predicates
            .iter()
            .filter(|p| listed.contains(p))
            .collect();
        if present.is_empty() {
            return mk(
                Verdict::Empty,
                format!(
                    "the body pins VALUES ?p to {} , none of which this dataset uses \
                     (it links with {})",
                    listed.join(" "),
                    card.signals.link_predicates.join(" ")
                ),
                revision,
                &binds,
            );
        }
    }

    // --- 6. "Which predicates point OUT?" needs something outside the base IRI.
    if q.id == "lk-external" && !caps.external_iri {
        let why = "no IRI the card records lies outside the dataset's base IRI".to_string();
        return if complete(card, &card.in_hubs) && complete(card, &card.classes) {
            mk(
                Verdict::Empty,
                format!("{why}, and both lists are complete — the FILTER keeps nothing"),
                revision,
                &binds,
            )
        } else {
            mk(
                Verdict::Suspect,
                format!("{why}, but the lists are truncated so one could have fallen off"),
                revision,
                &binds,
            )
        };
    }

    // --- 7. Two independently-ranked terms conjoined, with nothing in the card
    //        able to relate them: the shape, without a decision.
    let ranked = distinct_ranked(card, &binds);
    if ranked.len() > 1 {
        return mk(
            Verdict::Undecidable,
            format!(
                "conjoins {} , drawn from different rankings — the card relates no \
                 specific subject to a specific predicate",
                ranked.join(" and ")
            ),
            revision,
            &binds,
        );
    }

    // --- 8. An un-grouped aggregate that joins two predicates on one subject
    //        always returns its row; whether the row says anything is another
    //        matter, and one no card can settle (`class_links` collapses every
    //        untyped subject into one bucket, so rows for the two predicates
    //        under it are not evidence that any single subject carries both).
    if matches!(t.nonempty, NonEmpty::Aggregate)
        && body.contains(" ; ")
        && !body.contains("GROUP BY")
    {
        return mk(
            Verdict::Undecidable,
            "an un-grouped aggregate always returns one row, but its body conjoins two \
             predicates the card only counted separately — the row may be vacuous"
                .to_string(),
            revision,
            &binds,
        );
    }

    // --- 9. Nothing above bit: the template's own claim stands.
    match t.nonempty {
        NonEmpty::Undecidable(reason) => {
            mk(Verdict::Undecidable, reason.to_string(), revision, &binds)
        }
        NonEmpty::AnyGraph | NonEmpty::Aggregate | NonEmpty::Witnessed => mk(
            Verdict::Answers,
            match t.nonempty {
                NonEmpty::AnyGraph => "any non-empty graph answers this",
                NonEmpty::Aggregate => "an un-grouped aggregate always returns one row",
                _ => "every substituted term came from a count the card records as > 0",
            }
            .to_string(),
            revision,
            &binds,
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::card_derive::{derive_card, CardInput};
    use crate::{eval_query, ingest, summary_query_shape, QueryOutput, Rete};

    const TYPE: &str = "<http://www.w3.org/1999/02/22-rdf-syntax-ns#type>";

    fn q(s: &str, p: &str, o: &str) -> (String, String, String, Option<String>) {
        (s.into(), p.into(), o.into(), None)
    }

    /// Ids whose template admits it cannot prove non-emptiness. The allow-list
    /// is **read off the table** rather than written out here, so a template
    /// cannot quietly join it: adding `NonEmpty::Undecidable` to one means
    /// writing down the reason, in the source, next to the body.
    fn may_be_empty(id: &str) -> bool {
        claim_of(id) == Claim::Undecidable
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
                // No catch-all arm: `QueryOutput`'s `#[non_exhaustive]` does not
                // apply inside its own crate, so the three arms above are
                // exhaustive here — and a fourth result form should red this
                // test until someone decides what the query library does with it.
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

    // ---------------------------------------------------------------------
    // The audit: reading a card that is already published.
    // ---------------------------------------------------------------------

    /// Replace one starter query's body with the one an older revision wrote —
    /// how a fixture stands in for a file that is already on the wire.
    fn ship_instead(card: &mut DatasetCard, id: &str, body: &str) {
        let q = card
            .queries
            .iter_mut()
            .find(|q| q.id == id)
            .unwrap_or_else(|| panic!("{id} is not in this card"));
        q.sparql = format!("{PREFIXES}\n{body}");
    }

    fn verdict_of(findings: &[Finding], id: &str) -> Verdict {
        findings
            .iter()
            .find(|f| f.id == id)
            .unwrap_or_else(|| panic!("{id} was not audited"))
            .verdict
    }

    fn rows_of(rete: &Rete, sparql: &str) -> usize {
        match eval_query(rete, sparql).unwrap() {
            QueryOutput::Select(_, rows) => rows.len(),
            QueryOutput::Ask(b) => usize::from(b),
            other => panic!("unexpected result form: {other:?}"),
        }
    }

    /// A card this generator just wrote must audit clean: nothing refuted,
    /// nothing suspected, nothing unrecognized, and every body current. If the
    /// audit and the generator ever disagree about a *fresh* card, one of them
    /// is wrong — and they are supposed to share the judgement.
    #[test]
    fn a_freshly_generated_card_audits_clean() {
        for (quads, ng) in [(rich_quads(), 0), (named_only_quads(), 3)] {
            let card = derive_card(&quads, 50, ng, CardInput::default());
            for f in audit(&card) {
                assert_eq!(
                    f.revision, "current",
                    "{}: the generator would not write this body",
                    f.id
                );
                assert!(
                    matches!(f.verdict, Verdict::Answers | Verdict::Undecidable),
                    "{}: {} — {}",
                    f.id,
                    f.verdict.as_str(),
                    f.why
                );
                // The only queries a fresh card may leave undecided are the
                // ones the table already says it cannot decide, plus `sp-bbox`:
                // its un-grouped aggregate always returns its row (so
                // `NonEmpty::Aggregate` is true about *rows*), and whether that
                // row is bound turns on two predicates meeting on one subject,
                // which is the one thing `class_links` cannot witness.
                if f.verdict == Verdict::Undecidable {
                    assert!(
                        may_be_empty(&f.id) || f.id == "sp-bbox",
                        "{}: undecidable but the table claims otherwise — {}",
                        f.id,
                        f.why
                    );
                }
            }
        }
    }

    /// The **mtg** card as published: the label query joins the most populous
    /// class to the most-used label predicate. The audit refutes it, and the
    /// engine agrees — the verdict is checked against the graph, not just
    /// against the card.
    #[test]
    fn the_audit_refutes_a_label_query_on_an_unlabelled_top_class() {
        let quads = mtg_shaped_quads();
        let (mut card, rete) = card_and_graph(quads, 60, 0);
        let top = card.classes[0].0.clone();
        ship_instead(
            &mut card,
            "lb-labels",
            &format!("SELECT ?s ?label WHERE {{ ?s a {top} ; <http://schema.org/name> ?label }} LIMIT 50"),
        );
        ship_instead(
            &mut card,
            "cmp-coverage",
            &format!(
                "SELECT (COUNT(?s) AS ?total) (COUNT(?l) AS ?have) WHERE {{ ?s a {top} \
                 OPTIONAL {{ ?s <http://schema.org/name> ?l }} }}"
            ),
        );
        let findings = audit(&card);
        assert_eq!(verdict_of(&findings, "lb-labels"), Verdict::Empty);
        // The coverage query is an un-grouped aggregate: it returns its row, and
        // the row says `have = 0`. A different failure, reported as one.
        assert_eq!(verdict_of(&findings, "cmp-coverage"), Verdict::Vacuous);
        assert_eq!(rows_of(&rete, sparql_of(&card, "lb-labels").unwrap()), 0);
        assert_eq!(rows_of(&rete, sparql_of(&card, "cmp-coverage").unwrap()), 1);
    }

    /// The **vidy** shape, and the one false positive this audit was caught
    /// making: every unit is typed *twice* — `schema:ArchiveComponent` and a
    /// native class — so the `class_links` quotient, which files each subject
    /// under its last type only, shows no label row for the top class even
    /// though the label query returns rows.
    ///
    /// The audit must NOT claim a refutation here. Both tells are present (the
    /// top class has no row of its own; another class has exactly its instance
    /// count), and a survey that called this "provably empty" would have sent
    /// someone to re-card a file that was fine.
    #[test]
    fn the_audit_will_not_refute_a_dually_typed_class() {
        let name = "<http://schema.org/name>";
        let component = "<http://ex/ArchiveComponent>";
        let unit = "<http://ex/Unit>";
        let mut quads = Vec::new();
        for i in 0..8 {
            let s = format!("<http://ex/unit/{i}>");
            // Both types, the native one last — so the quotient files every
            // statement under `Unit` and says nothing about `ArchiveComponent`.
            quads.push(q(&s, TYPE, component));
            quads.push(q(&s, TYPE, unit));
            quads.push(q(&s, name, &format!("\"Unit {i}\"")));
        }
        let (mut card, rete) = card_and_graph(quads, 30, 0);
        assert_eq!(
            card.classes[0].0, component,
            "fixture: the twin ranks first"
        );
        assert!(
            !card.class_links.iter().any(|l| l.s_class == component),
            "fixture: the quotient never mentions the top class"
        );
        ship_instead(
            &mut card,
            "lb-labels",
            &format!("SELECT ?s ?label WHERE {{ ?s a {component} ; {name} ?label }} LIMIT 50"),
        );
        assert_eq!(verdict_of(&audit(&card), "lb-labels"), Verdict::Suspect);
        assert_eq!(rows_of(&rete, sparql_of(&card, "lb-labels").unwrap()), 8);
    }

    /// The **geoadmin** card as published: a fixed two-step geometry path on a
    /// file that hangs `geo:asWKT` straight off the feature. Nothing to do with
    /// typing — the path names a predicate the card's complete list does not
    /// contain, so the refutation is exact.
    #[test]
    fn the_audit_refutes_a_path_through_a_predicate_that_is_not_there() {
        let aswkt = "<http://www.opengis.net/ont/geosparql#asWKT>";
        let wkt = "http://www.opengis.net/ont/geosparql#wktLiteral";
        let mut quads = Vec::new();
        for i in 0..5 {
            let s = format!("<http://ex/district/{i}>");
            quads.push(q(&s, TYPE, "<http://ex/District>"));
            quads.push(q(&s, aswkt, &format!("\"POINT({} 4{i})\"^^<{wkt}>", i + 7)));
        }
        let (mut card, rete) = card_and_graph(quads, 20, 0);
        ship_instead(
            &mut card,
            "sp-within",
            "SELECT ?s WHERE { ?s geo:hasGeometry/geo:asWKT ?w FILTER(geof:sfWithin(?w, \
             \"POLYGON((-180 -90, 180 -90, 180 90, -180 90, -180 -90))\"^^geo:wktLiteral)) } LIMIT 100",
        );
        assert_eq!(verdict_of(&audit(&card), "sp-within"), Verdict::Empty);
        assert_eq!(rows_of(&rete, sparql_of(&card, "sp-within").unwrap()), 0);
    }

    /// A `VALUES` list that does not cover the gate admitting the query: the
    /// dataset links only with `skos:closeMatch`, the shipped body names the
    /// other three. Refutable exactly — `signals.link_predicates` is computed
    /// from the full predicate counts, before any cap.
    #[test]
    fn the_audit_refutes_a_values_list_that_misses_the_only_link_predicate() {
        let close = "<http://www.w3.org/2004/02/skos/core#closeMatch>";
        let mut quads = vec![q("<http://ex/a>", TYPE, "<http://ex/T>")];
        for i in 0..4 {
            quads.push(q(
                &format!("<http://ex/a{i}>"),
                close,
                &format!("<http://other/x{i}>"),
            ));
        }
        let (mut card, rete) = card_and_graph(quads, 20, 0);
        assert_eq!(card.signals.link_predicates, vec![close.to_string()]);
        ship_instead(
            &mut card,
            "lk-sameas",
            "SELECT ?p (COUNT(*) AS ?n) WHERE { ?s ?p ?o VALUES ?p { owl:sameAs \
             skos:exactMatch rdfs:seeAlso } } GROUP BY ?p",
        );
        assert_eq!(verdict_of(&audit(&card), "lk-sameas"), Verdict::Empty);
        assert_eq!(rows_of(&rete, sparql_of(&card, "lk-sameas").unwrap()), 0);
    }

    /// The pre-#172 `top-reach` — a hub IRI and a top predicate, conjoined —
    /// is recognized (so it is not reported as an unreadable body) and honestly
    /// declared undecidable: nothing in a card ties a *subject* to a
    /// *predicate*. On the published catalog 6 of 26 sampled files answered
    /// nothing here, which is exactly what "undecidable" has to mean.
    #[test]
    fn the_audit_recognizes_the_legacy_reach_query_and_declines_to_decide() {
        let (mut card, _) = card_and_graph(rich_quads(), 50, 0);
        let hub = card.top_hubs[0].0.clone();
        let pred = card.predicates[0].0.clone();
        ship_instead(
            &mut card,
            "top-reach",
            &format!("SELECT ?y WHERE {{ {hub} {pred}+ ?y }} LIMIT 100"),
        );
        let f = audit(&card);
        let r = f.iter().find(|f| f.id == "top-reach").unwrap();
        assert_eq!(r.verdict, Verdict::Undecidable);
        assert_eq!(r.revision, "superseded");
        assert!(r.conjoined, "two independently-ranked terms in one body");
        assert_eq!(r.substitutions.len(), 2);
    }

    /// A body from no revision at all is reported as such rather than being
    /// force-fitted to the nearest pattern — a wrong binding would produce a
    /// confident wrong verdict, which is the failure mode a survey cannot have.
    #[test]
    fn an_unknown_body_is_not_force_fitted() {
        let (mut card, _) = card_and_graph(rich_quads(), 50, 0);
        ship_instead(
            &mut card,
            "lb-labels",
            "SELECT ?x WHERE { ?x ?y ?z } LIMIT 3",
        );
        assert_eq!(
            verdict_of(&audit(&card), "lb-labels"),
            Verdict::Unrecognized
        );
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
        for t in TEMPLATES {
            assert!(
                !t.hook_is_exact || t.provably_empty.is_some(),
                "{}: hook_is_exact without a hook to be exact about",
                t.id
            );
        }
        // Every legacy body names a template that still exists (otherwise the
        // audit could never reach it) and is genuinely different from the ones
        // that template writes now (otherwise it is dead weight).
        for l in LEGACY_BODIES {
            let t = TEMPLATES
                .iter()
                .find(|t| t.id == l.id)
                .unwrap_or_else(|| panic!("legacy body for unknown template {}", l.id));
            assert!(
                ![Some(t.body), t.named_body, t.mixed_body]
                    .into_iter()
                    .flatten()
                    .any(|b| b == l.body),
                "{}: legacy body is still a current one",
                l.id
            );
        }
    }
}
