//! Dataset Cards — an embeddable data-catalog record stored in a `.rete` file's
//! metadata section. A card carries **curated** metadata (title, license,
//! source, description, created, example queries) plus **auto-derived**
//! statistics (counts, top predicates and classes, vocabularies), serialized as
//! JSON. `rete-core` treats the section as an opaque blob; this module owns its
//! schema, derivation, and rendering.
//!
//! Surfaced by `rete card [--json]` and folded into `rete info`'s catalog view.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use rete_core::{Header, RDF_TYPE};

/// The embeddable dataset card. Curated fields come from `rete build` flags or a
/// `--card-file` JSON document; the statistics are derived from the data at build
/// time. Absent optional/empty fields are omitted from the JSON.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct DatasetCard {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub license: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created: Option<String>,

    pub triple_count: u64,
    pub quad_count: u64,
    pub named_graph_count: u64,
    pub term_count: u64,

    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub predicates: Vec<(String, u64)>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub classes: Vec<(String, u64)>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub vocabularies: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub example_queries: Vec<String>,

    // --- Enriched, auto-derived profile (all additive; `#[serde(default)]` so
    // older cards without these fields still deserialize). Every list is capped
    // (see `CARD_TOP_N`) and deterministically ordered so the card folds into a
    // reproducible content hash; `truncated` records whether any list was cut. ---
    /// `DATATYPE(o)` histogram over literal objects, descending by count. Keys are
    /// bracketed datatype IRIs (`<…#integer>`, `<…#langString>` for `@lang`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub datatypes: Vec<(String, u64)>,
    /// `LANG(o)` histogram over literal objects, descending by count. The empty
    /// string `""` counts untagged/typed literals.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub languages: Vec<(String, u64)>,
    /// The effective schema: `(s_class, predicate, o_class, count)` over the
    /// default graph, the same quotient as [`rete_core::schema_summary`]. Object
    /// classes use the `(literal)`/`(untyped)` sentinels of that function.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub class_links: Vec<ClassLink>,
    /// Top subjects by out-degree (how many statements they make), descending.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub top_hubs: Vec<(String, u64)>,
    /// Top non-literal objects by in-degree (how often referenced), descending.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub in_hubs: Vec<(String, u64)>,
    /// Detected affordances — which exploration query families the data supports
    /// and the vocabulary to instantiate them with.
    #[serde(default, skip_serializing_if = "Signals::is_empty")]
    pub signals: Signals,
    /// Auto-generated, tiered starter-query library — vetted SPARQL with the
    /// dataset's own vocabulary substituted in, each tagged with the cheapest
    /// tier that can answer it. The cold-start "what do I ask?" fix.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub queries: Vec<ExampleQuery>,
    /// OWL RL / RDFS coherence verdict, stamped at build time by `--reason`. Lets a
    /// remote reader learn the graph's coherence from the index-free card with zero
    /// compute; `rete reason --verify-card` recomputes it to guard against drift.
    #[serde(default, skip_serializing_if = "Coherence::is_empty")]
    pub coherence: Coherence,
    /// Set iff any capped list was actually truncated (the profile is partial).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub truncated: bool,

    pub format_version: u8,
}

/// Which tier of the three-tier exploration model can answer a query cheapest:
/// `Card` (precomputed in this metadata section, index-free), `Summary` (the
/// pyramid superedge totals, index-free), or `Index` (needs the triple index).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum Tier {
    Card,
    Summary,
    Index,
}

/// One vetted starter query, instantiated with the dataset's real vocabulary.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct ExampleQuery {
    /// Stable identifier (e.g. `ov-triples`).
    pub id: String,
    /// Short human title.
    pub title: String,
    /// Exploration dimension (overview/identity/labels/types/topology/…).
    pub dimension: String,
    /// The plain-language question a newcomer would ask.
    pub question: String,
    /// Full runnable SPARQL, PREFIX block included, placeholders substituted.
    pub sparql: String,
    /// The cheapest tier that can answer it.
    pub tier: Tier,
    /// Capability keys that had to be present to emit this query.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub requires: Vec<String>,
}

/// One edge of the effective schema graph: a `predicate` connecting subjects of
/// class `s_class` to objects of class `o_class`, with the instance `count`.
/// Mirrors [`rete_core::schema_summary`] rows (sentinels `(literal)`/`(untyped)`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct ClassLink {
    pub s_class: String,
    pub predicate: String,
    pub o_class: String,
    pub count: u64,
}

/// Detected affordances — the index-free hints a newcomer (or the query-library
/// generator) needs to know what the data supports and how to address it. Every
/// field is optional/empty when the corresponding signal is absent, so geo/time
/// query families are emitted only when the data actually has geometry/time.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct Signals {
    /// Most frequent labelling predicate (`rdfs:label`/`skos:prefLabel`/…).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label_predicate: Option<String>,
    /// Dominant non-empty language tag over literal objects.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_lang: Option<String>,
    /// Dominant subject namespace — the dataset's own base IRI.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_iri: Option<String>,
    /// Temporal predicates, ranked by frequency (most-used first).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub time_predicates: Vec<String>,
    /// Predicates whose objects are numeric (xsd integer/decimal/double/float),
    /// ranked by frequency — candidates for `MIN/AVG/MAX` value-range queries.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub numeric_predicates: Vec<String>,
    /// Cross-dataset link predicates present (`owl:sameAs`/`skos:exactMatch`/…).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub link_predicates: Vec<String>,
    /// Any `geo:asWKT` / `geo:wktLiteral` geometry present.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub geo_wkt: bool,
    /// Both `wgs84:lat` and `wgs84:long` present.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub geo_latlong: bool,
    /// MIN/MAX lexical extent over the dominant time predicate (same datatype).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub temporal_extent: Option<(String, String)>,
    /// `[minLon, minLat, maxLon, maxLat]` over wgs84 lat/long objects.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub spatial_bbox: Option<[f64; 4]>,
}

impl Signals {
    /// True when no affordance was detected (lets the whole block be omitted).
    fn is_empty(&self) -> bool {
        *self == Signals::default()
    }
}

/// The build-time OWL RL / RDFS coherence verdict, stamped into the card by
/// `rete build --reason`. Deterministic and free of free-text detail (only a
/// sorted `by_kind` histogram + short tags), so it folds into the file's content
/// hash without destabilizing it. `scope` records what was checked and `rules` the
/// ruleset version, so `coherent: true` can't be misread as a guarantee from a
/// different scope or rule set.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct Coherence {
    /// True iff the reasoner found no incoherent point.
    pub coherent: bool,
    /// Number of incoherent points found.
    pub inconsistency_count: u32,
    /// `(kind, count)` histogram of incoherent points, sorted by kind.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub by_kind: Vec<(String, u32)>,
    /// What was checked, e.g. `"default"` (the default graph).
    pub scope: String,
    /// The reasoner ruleset version (`rete_core::REASON_RULESET`).
    pub rules: String,
    /// True iff the inferred triples were also materialized into the file.
    pub materialized: bool,
}

impl Coherence {
    /// An unstamped card has the all-default block (omitted from the JSON).
    pub(crate) fn is_empty(&self) -> bool {
        *self == Coherence::default()
    }

    /// Build the verdict from a reasoning result over the default graph. The
    /// `by_kind` histogram is `BTreeMap`-sourced (sorted) and carries no free-text
    /// `Inconsistency::detail`, so the stamp is byte-stable across rebuilds.
    pub(crate) fn from_reasoning(r: &rete_core::Reasoning, materialized: bool) -> Self {
        let mut hist: BTreeMap<&str, u32> = BTreeMap::new();
        for inc in &r.inconsistencies {
            *hist.entry(inc.kind).or_default() += 1;
        }
        Coherence {
            coherent: r.inconsistencies.is_empty(),
            inconsistency_count: r.inconsistencies.len() as u32,
            by_kind: hist.into_iter().map(|(k, v)| (k.to_string(), v)).collect(),
            scope: "default".to_string(),
            rules: rete_core::REASON_RULESET.to_string(),
            materialized,
        }
    }
}

/// The curated subset, as supplied by a `--card-file` JSON document (every field
/// optional). CLI flags override whatever the file provides.
#[derive(Debug, Clone, Default, Deserialize)]
pub(crate) struct CardInput {
    pub title: Option<String>,
    pub description: Option<String>,
    pub license: Option<String>,
    pub source: Option<String>,
    pub created: Option<String>,
    #[serde(default)]
    pub example_queries: Vec<String>,
}

/// The `rete build` card flags, bundled for threading through `build()`.
#[derive(Debug, Clone, Default)]
pub(crate) struct CardArgs {
    /// `--card`: embed a card even with no curated fields (stats only).
    pub enabled: bool,
    /// `--card-file <path>`: JSON document of curated fields.
    pub file: Option<String>,
    pub title: Option<String>,
    pub license: Option<String>,
    pub source: Option<String>,
    pub description: Option<String>,
    pub created: Option<String>,
}

impl CardArgs {
    /// Did the user ask for a card at all? Any flag presence opts in; otherwise
    /// the build stays cardless (byte-identical to a no-card build).
    pub fn requested(&self) -> bool {
        self.enabled
            || self.file.is_some()
            || self.title.is_some()
            || self.license.is_some()
            || self.source.is_some()
            || self.description.is_some()
            || self.created.is_some()
    }
}

impl DatasetCard {
    /// Serialize to the JSON bytes stored in the metadata section (compact).
    pub(crate) fn to_json_bytes(&self) -> Vec<u8> {
        serde_json::to_vec(self).expect("DatasetCard serializes")
    }

    /// Parse a card from the metadata-section bytes.
    pub(crate) fn from_json_bytes(b: &[u8]) -> anyhow::Result<Self> {
        serde_json::from_slice(b).map_err(|e| anyhow::anyhow!("malformed dataset card: {e}"))
    }

    /// Stamp the build-time coherence verdict (consuming builder).
    pub(crate) fn with_coherence(mut self, r: &rete_core::Reasoning, materialized: bool) -> Self {
        self.coherence = Coherence::from_reasoning(r, materialized);
        self
    }
}

/// Resolve the curated fields: load `--card-file` (if any), then let explicit
/// flags override individual fields.
pub(crate) fn load_curated(args: &CardArgs) -> anyhow::Result<CardInput> {
    let mut c = match &args.file {
        Some(path) => {
            let text = std::fs::read_to_string(path)
                .map_err(|e| anyhow::anyhow!("reading --card-file {path}: {e}"))?;
            serde_json::from_str(&text)
                .map_err(|e| anyhow::anyhow!("parsing --card-file {path}: {e}"))?
        }
        None => CardInput::default(),
    };
    if args.title.is_some() {
        c.title = args.title.clone();
    }
    if args.license.is_some() {
        c.license = args.license.clone();
    }
    if args.source.is_some() {
        c.source = args.source.clone();
    }
    if args.description.is_some() {
        c.description = args.description.clone();
    }
    if args.created.is_some() {
        c.created = args.created.clone();
    }
    Ok(c)
}

/// Cap for every top-N list embedded in the card. The metadata section is
/// fetched on **every** overview (it is part of the index-free CARD tier), so an
/// unbounded `class_links` (O(classes × predicates × classes)) or predicate list
/// would bloat that fetch on a large schema (CIDOC-CRM/MMM). Capping keeps the
/// card small and bounded; `truncated` flags when a list was actually cut.
const CARD_TOP_N: usize = 100;

// Well-known IRIs (bracketed N-Triples term form, as they appear in the quads).
const RDFS_LABEL: &str = "<http://www.w3.org/2000/01/rdf-schema#label>";
const SKOS_PREFLABEL: &str = "<http://www.w3.org/2004/02/skos/core#prefLabel>";
const SCHEMA_NAME: &str = "<http://schema.org/name>";
const FOAF_NAME: &str = "<http://xmlns.com/foaf/0.1/name>";
const DCT_TITLE: &str = "<http://purl.org/dc/terms/title>";
const OWL_SAMEAS: &str = "<http://www.w3.org/2002/07/owl#sameAs>";
const SKOS_EXACTMATCH: &str = "<http://www.w3.org/2004/02/skos/core#exactMatch>";
const SKOS_CLOSEMATCH: &str = "<http://www.w3.org/2004/02/skos/core#closeMatch>";
const RDFS_SEEALSO: &str = "<http://www.w3.org/2000/01/rdf-schema#seeAlso>";
const WGS_LAT: &str = "<http://www.w3.org/2003/01/geo/wgs84_pos#lat>";
const WGS_LONG: &str = "<http://www.w3.org/2003/01/geo/wgs84_pos#long>";
const GEO_ASWKT: &str = "<http://www.opengis.net/ont/geosparql#asWKT>";
const GEO_HASGEOMETRY: &str = "<http://www.opengis.net/ont/geosparql#hasGeometry>";
// Datatype IRIs (unbracketed — as they appear after `^^` in a literal term).
const GEO_WKTLITERAL: &str = "http://www.opengis.net/ont/geosparql#wktLiteral";
const XSD_STRING: &str = "http://www.w3.org/2001/XMLSchema#string";
const RDF_LANGSTRING: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#langString";

/// Label predicates in priority order; the most frequent **present** one wins.
const LABEL_PREDICATES: &[&str] = &[
    SKOS_PREFLABEL,
    RDFS_LABEL,
    SCHEMA_NAME,
    FOAF_NAME,
    DCT_TITLE,
];
/// Cross-dataset linking predicates (emitted in `signals.link_predicates`).
const LINK_PREDICATES: &[&str] = &[OWL_SAMEAS, SKOS_EXACTMATCH, SKOS_CLOSEMATCH, RDFS_SEEALSO];

/// A replayable source of a graph's **default-graph** triples for card
/// derivation. `replay` may be called more than once (the derivation makes two
/// passes); every call MUST yield the same triples in the same order. Terms are
/// passed as borrowed `&str` valid only for that call — so the derivation owns
/// (clones) any key it retains, letting one code path serve both an in-memory
/// quad slice and a streaming build's dictionary + id-triples.
pub(crate) trait CardTripleSource {
    fn replay(&self, f: &mut dyn FnMut(&str, &str, &str));
}

/// Derive a full card from the parsed quads plus a few build-time counts — the
/// in-memory build path. Statistics are over the **default graph** only
/// (named-graph statistics are summarized by `quad_count`/`named_graph_count`),
/// matching `rete stats`/`rete predicates`.
pub(crate) fn derive_card(
    quads: &[(String, String, String, Option<String>)],
    term_count: u64,
    named_graph_count: u64,
    curated: CardInput,
) -> DatasetCard {
    struct Quads<'a>(&'a [(String, String, String, Option<String>)]);
    impl CardTripleSource for Quads<'_> {
        fn replay(&self, f: &mut dyn FnMut(&str, &str, &str)) {
            for (s, p, o, g) in self.0 {
                if g.is_none() {
                    f(s, p, o);
                }
            }
        }
    }
    derive_card_from(
        &Quads(quads),
        quads.len() as u64,
        term_count,
        named_graph_count,
        curated,
    )
}

/// Derive a card from a built dictionary + default-graph **id-triples** — the
/// streaming (low-RAM) build path, where the raw quads were never retained.
/// Resolves each id-triple back to its terms through the dictionary, yielding the
/// same `(s, p, o)` strings in file order, so the card is **byte-identical** to
/// the in-memory derivation on the same graph.
pub(crate) fn derive_card_encoded(
    dict: &rete_core::Dictionary,
    triples: &[(u32, u32, u32)],
    quad_count: u64,
    term_count: u64,
    named_graph_count: u64,
    curated: CardInput,
) -> DatasetCard {
    struct Encoded<'a> {
        dict: &'a rete_core::Dictionary,
        triples: &'a [(u32, u32, u32)],
    }
    impl CardTripleSource for Encoded<'_> {
        fn replay(&self, f: &mut dyn FnMut(&str, &str, &str)) {
            for &(s, p, o) in self.triples {
                let st = self.dict.subject_term(s).unwrap_or_default();
                let pt = self.dict.predicate_term(p).unwrap_or_default();
                let ot = self.dict.object_term(o).unwrap_or_default();
                f(&st, &pt, &ot);
            }
        }
    }
    derive_card_from(
        &Encoded { dict, triples },
        quad_count,
        term_count,
        named_graph_count,
        curated,
    )
}

/// The shared card derivation, over any [`CardTripleSource`]. Two passes: the
/// first builds the subject→class map needed to classify both endpoints of every
/// relation (the `class_links` quotient — the same logic as
/// [`rete_core::schema_summary`], folded in here to avoid a second full
/// materialization); the second tallies everything else. All counts are over the
/// raw (pre-dedup) default-graph multiset, matching the existing card stats and
/// `rete progressive`.
fn derive_card_from(
    src: &dyn CardTripleSource,
    quad_count: u64,
    term_count: u64,
    named_graph_count: u64,
    curated: CardInput,
) -> DatasetCard {
    // --- Pass 1: subject → class (last type wins, matching `schema_summary`). ---
    let mut class_of: std::collections::HashMap<String, String> = std::collections::HashMap::new();
    src.replay(&mut |s, p, o| {
        if p == RDF_TYPE {
            class_of.insert(s.to_string(), o.to_string());
        }
    });
    let classify = |t: &str| -> String {
        if let Some(c) = class_of.get(t) {
            c.to_string()
        } else if t.starts_with('"') {
            "(literal)".to_string()
        } else {
            "(untyped)".to_string()
        }
    };

    // --- Pass 2: every other statistic, in one sweep. ---
    // Keys are owned `String`s (not `&str` borrowed from a quad slice) so the
    // same derivation serves a streaming source whose term strings are transient;
    // a `BTreeMap<String, _>` sorts identically to the old `BTreeMap<&str, _>`, so
    // the output is byte-identical.
    let mut pred_counts: BTreeMap<String, u64> = BTreeMap::new();
    let mut class_counts: BTreeMap<String, u64> = BTreeMap::new();
    let mut datatype_counts: BTreeMap<String, u64> = BTreeMap::new();
    let mut lang_counts: BTreeMap<String, u64> = BTreeMap::new();
    let mut out_degree: BTreeMap<String, u64> = BTreeMap::new();
    let mut in_degree: BTreeMap<String, u64> = BTreeMap::new();
    let mut link_rows: BTreeMap<(String, String, String), u64> = BTreeMap::new();
    let mut subject_ns: BTreeMap<String, u64> = BTreeMap::new();
    // Per-predicate temporal evidence: object-shape hits and a name hint.
    let mut time_obj_hits: BTreeMap<String, u64> = BTreeMap::new();
    // Per-predicate numeric-object hits (for value-range queries).
    let mut num_obj_hits: BTreeMap<String, u64> = BTreeMap::new();
    // Objects of each candidate time predicate, grouped by datatype, for extent.
    let mut time_values: BTreeMap<String, (Option<String>, Option<String>)> = BTreeMap::new();
    let mut time_value_dt: BTreeMap<String, String> = BTreeMap::new();
    let mut lat_lo = f64::INFINITY;
    let mut lat_hi = f64::NEG_INFINITY;
    let mut lon_lo = f64::INFINITY;
    let mut lon_hi = f64::NEG_INFINITY;
    let mut have_lat = false;
    let mut have_lon = false;
    let mut geo_wkt = false;
    let mut triple_count = 0u64;

    src.replay(&mut |s, p, o| {
        triple_count += 1;
        *pred_counts.entry(p.to_string()).or_default() += 1;
        *out_degree.entry(s.to_string()).or_default() += 1;
        if let Some(ns) = split_namespace(s) {
            *subject_ns.entry(ns).or_default() += 1;
        }

        // In-degree: every non-literal object referenced. Matches the
        // `top-in-hubs` query (`FILTER(!isLiteral(?o))`), which does not
        // special-case rdf:type, so the card precompute equals the query result.
        if !o.starts_with('"') {
            *in_degree.entry(o.to_string()).or_default() += 1;
        }

        if p == RDF_TYPE {
            *class_counts.entry(o.to_string()).or_default() += 1;
            return; // type assertions define classes, not data relations
        }

        // Class-to-class quotient (the effective schema).
        *link_rows
            .entry((classify(s), p.to_string(), classify(o)))
            .or_default() += 1;

        if p == GEO_ASWKT || p == GEO_HASGEOMETRY {
            geo_wkt = true;
        }

        // Literal objects: datatype / language / temporal / geo analysis.
        if let Some(lit) = parse_literal(o) {
            *datatype_counts
                .entry(format!("<{}>", lit.datatype))
                .or_default() += 1;
            *lang_counts.entry(lit.lang.clone()).or_default() += 1;
            if lit.datatype == GEO_WKTLITERAL {
                geo_wkt = true;
            }
            // Temporal object-shape: a date/year datatype or a year-like value.
            if is_temporal_datatype(&lit.datatype) || looks_like_year(&lit.value) {
                *time_obj_hits.entry(p.to_string()).or_default() += 1;
                update_time_extent(p, &lit, &mut time_values, &mut time_value_dt);
            } else if is_numeric_datatype(&lit.datatype) {
                // A year-shaped value is temporal, not "numeric range" material,
                // so this is an `else if` — `birthYear` is a time predicate.
                *num_obj_hits.entry(p.to_string()).or_default() += 1;
            }
            // wgs84 lat/long numeric extent.
            if p == WGS_LAT {
                if let Ok(v) = lit.value.parse::<f64>() {
                    have_lat = true;
                    lat_lo = lat_lo.min(v);
                    lat_hi = lat_hi.max(v);
                }
            } else if p == WGS_LONG {
                if let Ok(v) = lit.value.parse::<f64>() {
                    have_lon = true;
                    lon_lo = lon_lo.min(v);
                    lon_hi = lon_hi.max(v);
                }
            }
        }
    });

    // Signal lookups that need the raw per-predicate counts — computed before
    // `pred_counts` is consumed by the sort below.
    let pred_count = |iri: &str| pred_counts.get(iri).copied().unwrap_or(0);
    let label_predicate = LABEL_PREDICATES
        .iter()
        .filter(|&&iri| pred_count(iri) > 0)
        .max_by_key(|&&iri| pred_count(iri))
        .map(|s| s.to_string());
    let link_predicates: Vec<String> = LINK_PREDICATES
        .iter()
        .filter(|&&iri| pred_count(iri) > 0)
        .map(|s| s.to_string())
        .collect();

    let mut truncated = false;
    let predicates = cap(sort_desc_owned(pred_counts), &mut truncated);
    let classes = cap(sort_desc_owned(class_counts), &mut truncated);
    let datatypes = cap(sort_desc_owned(datatype_counts), &mut truncated);
    let languages = cap(sort_desc_owned(lang_counts), &mut truncated);
    let top_hubs = cap(sort_desc_owned(out_degree), &mut truncated);
    let in_hubs = cap(sort_desc_owned(in_degree), &mut truncated);
    let class_links = cap(sort_links(link_rows), &mut truncated);

    // Vocabularies: distinct namespaces of the predicate and class IRIs.
    let mut vocab: BTreeSet<String> = BTreeSet::new();
    for (iri, _) in predicates.iter().chain(classes.iter()) {
        if let Some(ns) = split_namespace(iri) {
            vocab.insert(ns);
        }
    }
    let vocabularies = cap(vocab.into_iter().collect(), &mut truncated);

    // --- Signals (index-free affordances for the query-library generator). ---
    let default_lang = languages
        .iter()
        .find(|(l, _)| !l.is_empty())
        .map(|(l, _)| l.clone());
    let base_iri = derive_base_iri(&subject_ns);
    let mut time_ranked: Vec<(String, u64)> = time_obj_hits.into_iter().collect();
    time_ranked.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    // Cap through the shared helper so an over-long list also flips `truncated`.
    let time_predicates = cap(
        time_ranked.iter().map(|(p, _)| p.to_string()).collect(),
        &mut truncated,
    );
    let mut num_ranked: Vec<(String, u64)> = num_obj_hits.into_iter().collect();
    num_ranked.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    let numeric_predicates = cap(
        num_ranked.iter().map(|(p, _)| p.to_string()).collect(),
        &mut truncated,
    );
    let geo_latlong = have_lat && have_lon;
    let temporal_extent = time_ranked
        .first()
        .and_then(|(p, _)| time_values.get(p))
        .and_then(|(lo, hi)| match (lo, hi) {
            (Some(lo), Some(hi)) => Some((lo.clone(), hi.clone())),
            _ => None,
        });
    let spatial_bbox = if geo_latlong && lon_lo.is_finite() && lat_lo.is_finite() {
        Some([lon_lo, lat_lo, lon_hi, lat_hi])
    } else {
        None
    };

    let signals = Signals {
        label_predicate,
        default_lang,
        base_iri,
        time_predicates,
        numeric_predicates,
        link_predicates,
        geo_wkt,
        geo_latlong,
        temporal_extent,
        spatial_bbox,
    };

    let mut card = DatasetCard {
        title: curated.title,
        description: curated.description,
        license: curated.license,
        source: curated.source,
        created: curated.created,
        triple_count,
        quad_count,
        named_graph_count,
        term_count,
        predicates,
        classes,
        vocabularies,
        example_queries: curated.example_queries,
        datatypes,
        languages,
        class_links,
        top_hubs,
        in_hubs,
        signals,
        queries: Vec::new(),
        // Stamped later by the build pipeline via `with_coherence` when `--reason`
        // (or `--materialize`) ran; derive_card itself must not run the reasoner.
        coherence: Coherence::default(),
        truncated,
        format_version: rete_core::VERSION,
    };
    // The tiered starter-query library, instantiated from the profile above.
    card.queries = super::queries::generate(&card);
    card
}

/// Cap a top-N list, flagging `truncated` if anything was dropped.
fn cap<T>(mut v: Vec<T>, truncated: &mut bool) -> Vec<T> {
    if v.len() > CARD_TOP_N {
        v.truncate(CARD_TOP_N);
        *truncated = true;
    }
    v
}

/// Sort an owned-`String`-keyed count map descending by count, then by term.
fn sort_desc_owned(counts: BTreeMap<String, u64>) -> Vec<(String, u64)> {
    let mut v: Vec<(String, u64)> = counts.into_iter().collect();
    v.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    v
}

/// Sort the class-link quotient deterministically: descending by count, then by
/// the `(s_class, predicate, o_class)` key — a total order for a stable hash.
fn sort_links(rows: BTreeMap<(String, String, String), u64>) -> Vec<ClassLink> {
    let mut v: Vec<ClassLink> = rows
        .into_iter()
        .map(|((s, p, o), count)| ClassLink {
            s_class: s,
            predicate: p,
            o_class: o,
            count,
        })
        .collect();
    v.sort_by(|a, b| {
        b.count
            .cmp(&a.count)
            .then_with(|| a.s_class.cmp(&b.s_class))
            .then_with(|| a.predicate.cmp(&b.predicate))
            .then_with(|| a.o_class.cmp(&b.o_class))
    });
    v
}

/// The dataset's own base IRI, for the `{{BASE_IRI}}` substitution (and the
/// "links pointing outside the dataset" query). The longest common prefix of all
/// subject namespaces, truncated to a `/`/`#` boundary — `http://ex/` for
/// subjects under `http://ex/person/`, `http://ex/place/`, … When the LCP is
/// coarser than `scheme://host/` (subjects span unrelated hosts), fall back to
/// the single most-frequent namespace so the value stays a usable prefix.
fn derive_base_iri(ns: &BTreeMap<String, u64>) -> Option<String> {
    if ns.is_empty() {
        return None;
    }
    // Dominant namespace: highest count, ties broken by the smaller string.
    let dominant = ns
        .iter()
        .max_by(|a, b| a.1.cmp(b.1).then_with(|| b.0.cmp(a.0)))
        .map(|(k, _)| k.clone())?;
    // Longest common prefix across every subject namespace.
    let mut lcp_len = dominant.len();
    for k in ns.keys() {
        let common = dominant
            .bytes()
            .zip(k.bytes())
            .take_while(|(a, b)| a == b)
            .count();
        lcp_len = lcp_len.min(common);
    }
    let lcp = &dominant[..lcp_len];
    // Truncate to the last namespace delimiter so we never cut mid-segment.
    let base = match lcp.rfind(['/', '#']) {
        Some(i) => &lcp[..=i],
        None => "",
    };
    // Require at least `scheme://host/` (three slashes); else the LCP collapsed
    // across hosts — the dominant namespace is the more useful prefix.
    if base.matches('/').count() >= 3 {
        Some(base.to_string())
    } else {
        Some(dominant)
    }
}

/// The decomposed parts of an RDF literal term.
struct Literal {
    /// Lexical value with the surrounding quotes and escapes removed.
    value: String,
    /// Datatype IRI (unbracketed): `xsd:string` for plain, `rdf:langString` for
    /// language-tagged, else the explicit `^^<…>` type.
    datatype: String,
    /// Language tag (`""` when none).
    lang: String,
}

/// Decompose an N-Triples literal term (`"…"`, `"…"@lang`, `"…"^^<dt>`) into its
/// value, datatype, and language. Returns `None` for non-literal terms.
fn parse_literal(term: &str) -> Option<Literal> {
    let bytes = term.as_bytes();
    if bytes.first() != Some(&b'"') {
        return None;
    }
    // Find the closing unescaped quote.
    let mut i = 1usize;
    let mut esc = false;
    let mut close = None;
    while i < bytes.len() {
        match bytes[i] {
            b'\\' => esc = !esc,
            b'"' if !esc => {
                close = Some(i);
                break;
            }
            _ => esc = false,
        }
        i += 1;
    }
    let close = close?;
    let value = term[1..close].to_string();
    let suffix = &term[close + 1..];
    if let Some(lang) = suffix.strip_prefix('@') {
        Some(Literal {
            value,
            datatype: RDF_LANGSTRING.to_string(),
            lang: lang.to_string(),
        })
    } else if let Some(dt) = suffix.strip_prefix("^^") {
        let dt = dt.trim_start_matches('<').trim_end_matches('>').to_string();
        Some(Literal {
            value,
            datatype: dt,
            lang: String::new(),
        })
    } else {
        Some(Literal {
            value,
            datatype: XSD_STRING.to_string(),
            lang: String::new(),
        })
    }
}

/// True for XSD date/time/year datatypes — the strong temporal signal.
fn is_temporal_datatype(dt: &str) -> bool {
    matches!(
        dt,
        "http://www.w3.org/2001/XMLSchema#date"
            | "http://www.w3.org/2001/XMLSchema#dateTime"
            | "http://www.w3.org/2001/XMLSchema#gYear"
            | "http://www.w3.org/2001/XMLSchema#gYearMonth"
            | "http://www.w3.org/2001/XMLSchema#gMonthDay"
    )
}

/// True for XSD numeric datatypes — candidates for value-range aggregates.
fn is_numeric_datatype(dt: &str) -> bool {
    matches!(
        dt,
        "http://www.w3.org/2001/XMLSchema#integer"
            | "http://www.w3.org/2001/XMLSchema#decimal"
            | "http://www.w3.org/2001/XMLSchema#double"
            | "http://www.w3.org/2001/XMLSchema#float"
            | "http://www.w3.org/2001/XMLSchema#long"
            | "http://www.w3.org/2001/XMLSchema#int"
            | "http://www.w3.org/2001/XMLSchema#short"
            | "http://www.w3.org/2001/XMLSchema#nonNegativeInteger"
            | "http://www.w3.org/2001/XMLSchema#positiveInteger"
    )
}

/// A weak temporal signal: a value whose leading token is a 4-digit year
/// (optionally BCE-negative), e.g. `1492`, `1492-10-12`, `-0044-03-15`.
fn looks_like_year(value: &str) -> bool {
    let v = value.strip_prefix('-').unwrap_or(value);
    let head: String = v.chars().take(4).collect();
    head.len() == 4 && head.chars().all(|c| c.is_ascii_digit())
}

/// Fold one temporal literal into the per-predicate MIN/MAX extent, keeping the
/// extent within a single datatype (the predicate's first-seen one) so a lexical
/// compare never mixes incompatible value spaces (Risk R4).
fn update_time_extent(
    pred: &str,
    lit: &Literal,
    values: &mut BTreeMap<String, (Option<String>, Option<String>)>,
    value_dt: &mut BTreeMap<String, String>,
) {
    let dt = value_dt
        .entry(pred.to_string())
        .or_insert_with(|| lit.datatype.clone());
    if *dt != lit.datatype {
        return; // a different value space for this predicate — skip for extent
    }
    let entry = values.entry(pred.to_string()).or_insert((None, None));
    match &mut entry.0 {
        Some(lo) if lit.value < *lo => *lo = lit.value.clone(),
        None => entry.0 = Some(lit.value.clone()),
        _ => {}
    }
    match &mut entry.1 {
        Some(hi) if lit.value > *hi => *hi = lit.value.clone(),
        None => entry.1 = Some(lit.value.clone()),
        _ => {}
    }
}

/// The namespace of an IRI term: the prefix up to and including the last `#` or
/// `/`. Returns `None` for non-IRI terms (literals/bnodes) or IRIs with neither
/// delimiter. Best-effort — adequate for a descriptive vocabulary list.
fn split_namespace(term: &str) -> Option<String> {
    let iri = term.strip_prefix('<')?.strip_suffix('>')?;
    let cut = iri.rfind(['#', '/'])?;
    Some(iri[..=cut].to_string())
}

/// Hex-encode a 16-byte content hash (the `.rete` integrity checksum).
pub(crate) fn hex16(bytes: &[u8; 16]) -> String {
    let mut s = String::with_capacity(32);
    for b in bytes {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

/// Read the dataset card embedded in a `.rete` file image, or `None` if it has
/// no metadata section. Reads only the header + metadata range (never decodes
/// the dictionary/index).
pub(crate) fn load_card(bytes: &[u8]) -> anyhow::Result<Option<DatasetCard>> {
    let header = Header::from_bytes(bytes)?;
    if header.metadata_len == 0 {
        return Ok(None);
    }
    let start = header.metadata_offset as usize;
    let end = start
        .checked_add(header.metadata_len as usize)
        .filter(|&e| e <= bytes.len())
        .ok_or_else(|| anyhow::anyhow!("metadata section out of bounds"))?;
    Ok(Some(DatasetCard::from_json_bytes(&bytes[start..end])?))
}

/// Read the dataset card via a [`rete_core::RangeReader`] — fetching only the
/// header and metadata range (the index-free CARD tier), never the
/// dictionary/index/pyramid. The remote/S3 companion to [`load_card`], which
/// needs the whole file in memory. Returns `None` when the file has no card.
pub(crate) fn load_card_ranged<R: rete_core::RangeReader>(
    reader: &R,
) -> anyhow::Result<Option<DatasetCard>> {
    match rete_core::read_metadata_ranged(reader)? {
        None => Ok(None),
        Some(bytes) => Ok(Some(DatasetCard::from_json_bytes(&bytes)?)),
    }
}

/// Render a card as a human-readable catalog. `checksum` is the file's content
/// hash in hex (the integrity checksum surfaced by `rete verify`).
pub(crate) fn format_card(card: &DatasetCard, checksum: &str) -> String {
    use std::fmt::Write;
    let mut out = String::new();
    let _ = writeln!(out, "Dataset Card");
    let field = |out: &mut String, label: &str, value: &Option<String>| {
        if let Some(v) = value {
            let _ = writeln!(out, "  {label:<13}: {v}");
        }
    };
    field(&mut out, "title", &card.title);
    field(&mut out, "description", &card.description);
    field(&mut out, "license", &card.license);
    field(&mut out, "source", &card.source);
    field(&mut out, "created", &card.created);

    let _ = writeln!(out, "  {:<13}: {}", "triples", card.triple_count);
    if card.named_graph_count > 0 {
        let _ = writeln!(
            out,
            "  {:<13}: {} total, {} named graph(s)",
            "quads", card.quad_count, card.named_graph_count
        );
    }
    let _ = writeln!(out, "  {:<13}: {}", "terms", card.term_count);
    let _ = writeln!(
        out,
        "  {:<13}: {checksum}  (blake3-16 content hash)",
        "checksum"
    );

    if !card.vocabularies.is_empty() {
        let _ = writeln!(out, "  {:<13}: {}", "vocabularies", card.vocabularies.len());
        for ns in &card.vocabularies {
            let _ = writeln!(out, "      {ns}");
        }
    }
    if !card.predicates.is_empty() {
        let _ = writeln!(out, "  predicates ({}):", card.predicates.len());
        for (iri, count) in &card.predicates {
            let _ = writeln!(out, "      {count:>8}  {iri}");
        }
    }
    if !card.classes.is_empty() {
        let _ = writeln!(out, "  classes ({}):", card.classes.len());
        for (iri, count) in &card.classes {
            let _ = writeln!(out, "      {count:>8}  {iri}");
        }
    }
    if !card.datatypes.is_empty() {
        let _ = writeln!(out, "  datatypes ({}):", card.datatypes.len());
        for (dt, count) in &card.datatypes {
            let _ = writeln!(out, "      {count:>8}  {dt}");
        }
    }
    if !card.languages.is_empty() {
        let _ = writeln!(out, "  languages ({}):", card.languages.len());
        for (lang, count) in &card.languages {
            let shown = if lang.is_empty() { "(untagged)" } else { lang };
            let _ = writeln!(out, "      {count:>8}  {shown}");
        }
    }
    if !card.class_links.is_empty() {
        let _ = writeln!(out, "  class links ({}):", card.class_links.len());
        for l in &card.class_links {
            let _ = writeln!(
                out,
                "      {:>8}  {} --{}-> {}",
                l.count, l.s_class, l.predicate, l.o_class
            );
        }
    }
    if !card.top_hubs.is_empty() {
        let _ = writeln!(out, "  top hubs (out-degree):");
        for (iri, d) in &card.top_hubs {
            let _ = writeln!(out, "      {d:>8}  {iri}");
        }
    }
    if !card.in_hubs.is_empty() {
        let _ = writeln!(out, "  top hubs (in-degree):");
        for (iri, d) in &card.in_hubs {
            let _ = writeln!(out, "      {d:>8}  {iri}");
        }
    }
    if !card.signals.is_empty() {
        let s = &card.signals;
        let _ = writeln!(out, "  signals:");
        let opt = |out: &mut String, label: &str, v: &Option<String>| {
            if let Some(v) = v {
                let _ = writeln!(out, "      {label:<11}: {v}");
            }
        };
        opt(&mut out, "label pred", &s.label_predicate);
        opt(&mut out, "base IRI", &s.base_iri);
        opt(&mut out, "default lang", &s.default_lang);
        if !s.time_predicates.is_empty() {
            let _ = writeln!(out, "      time preds : {}", s.time_predicates.join(", "));
        }
        if !s.link_predicates.is_empty() {
            let _ = writeln!(out, "      link preds : {}", s.link_predicates.join(", "));
        }
        if let Some((from, to)) = &s.temporal_extent {
            let _ = writeln!(out, "      time extent: {from} … {to}");
        }
        if let Some([min_lon, min_lat, max_lon, max_lat]) = s.spatial_bbox {
            let _ = writeln!(
                out,
                "      bbox       : lon [{min_lon}, {max_lon}], lat [{min_lat}, {max_lat}] (CRS84 lon/lat)"
            );
        }
        if s.geo_wkt {
            let _ = writeln!(out, "      geometry   : geo:asWKT present");
        }
    }
    if !card.coherence.is_empty() {
        let c = &card.coherence;
        let verdict = if c.coherent {
            "coherent".to_string()
        } else {
            format!("{} incoherent point(s)", c.inconsistency_count)
        };
        let _ = writeln!(out, "  coherence:");
        let _ = writeln!(out, "      verdict    : {verdict}");
        if !c.by_kind.is_empty() {
            let kinds: Vec<String> = c.by_kind.iter().map(|(k, n)| format!("{k}×{n}")).collect();
            let _ = writeln!(out, "      by kind    : {}", kinds.join(", "));
        }
        let _ = writeln!(
            out,
            "      scope      : {} · rules {} · {}",
            c.scope,
            c.rules,
            if c.materialized {
                "materialized"
            } else {
                "not materialized"
            }
        );
    }
    if !card.queries.is_empty() {
        let _ = writeln!(out, "  starter queries ({}):", card.queries.len());
        for q in &card.queries {
            let tier = match q.tier {
                Tier::Card => "card",
                Tier::Summary => "summary",
                Tier::Index => "index",
            };
            let _ = writeln!(out, "      [{tier:^7}] {} — {}", q.id, q.question);
        }
    }
    if !card.example_queries.is_empty() {
        let _ = writeln!(out, "  curated queries:");
        for q in &card.example_queries {
            let _ = writeln!(out, "      {q}");
        }
    }
    // Trim the trailing newline so callers control spacing.
    while out.ends_with('\n') {
        out.pop();
    }
    out
}

/// `rete card <file> [--json]`: print the embedded dataset card (catalog view),
/// or the raw JSON with `--json`. Prints `(no dataset card)` when absent.
pub(crate) fn card_cmd(file: &str, json: bool) -> anyhow::Result<()> {
    let bytes = std::fs::read(file)?;
    let header = Header::from_bytes(&bytes)?;
    match load_card(&bytes)? {
        None => println!("(no dataset card)"),
        Some(card) => {
            if json {
                println!("{}", serde_json::to_string_pretty(&card)?);
            } else {
                println!("{}", format_card(&card, &hex16(&header.content_hash)));
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const TYPE: &str = RDF_TYPE;

    fn q(s: &str, p: &str, o: &str) -> (String, String, String, Option<String>) {
        (s.into(), p.into(), o.into(), None)
    }

    #[test]
    fn card_json_round_trips() {
        let card = DatasetCard {
            title: Some("Demo".into()),
            license: Some("CC0".into()),
            triple_count: 3,
            quad_count: 3,
            term_count: 5,
            predicates: vec![("<http://ex/p>".into(), 2)],
            vocabularies: vec!["http://ex/".into()],
            format_version: 1,
            ..Default::default()
        };
        let bytes = card.to_json_bytes();
        let back = DatasetCard::from_json_bytes(&bytes).unwrap();
        assert_eq!(card, back);
        // Absent optionals/empties are omitted from the JSON.
        let text = String::from_utf8(bytes).unwrap();
        assert!(!text.contains("description"));
        assert!(!text.contains("classes"));
    }

    #[test]
    fn split_namespace_handles_hash_slash_and_neither() {
        assert_eq!(
            split_namespace("<http://xmlns.com/foaf/0.1/name>").as_deref(),
            Some("http://xmlns.com/foaf/0.1/")
        );
        assert_eq!(
            split_namespace("<http://www.w3.org/1999/02/22-rdf-syntax-ns#type>").as_deref(),
            Some("http://www.w3.org/1999/02/22-rdf-syntax-ns#")
        );
        // No delimiter, a literal, and a bnode all yield None.
        assert_eq!(split_namespace("<urn:isbn:12345>"), None);
        assert_eq!(split_namespace("\"literal\""), None);
        assert_eq!(split_namespace("_:b0"), None);
    }

    #[test]
    fn derive_counts_and_orders() {
        let quads = vec![
            q("<http://ex/a>", TYPE, "<http://ex/Person>"),
            q("<http://ex/b>", TYPE, "<http://ex/Person>"),
            q("<http://ex/c>", TYPE, "<http://ex/City>"),
            q("<http://ex/a>", "<http://ex/knows>", "<http://ex/b>"),
            q("<http://ex/a>", "<http://ex/knows>", "<http://ex/c>"),
            q("<http://ex/a>", "<http://ex/name>", "\"A\""),
        ];
        let card = derive_card(&quads, 9, 0, CardInput::default());
        assert_eq!(card.triple_count, 6);
        assert_eq!(card.quad_count, 6);
        assert_eq!(card.term_count, 9);
        // Predicates desc by count: knows(2) and rdf:type(3) and name(1).
        assert_eq!(card.predicates[0], (TYPE.to_string(), 3));
        assert!(card
            .predicates
            .contains(&("<http://ex/knows>".to_string(), 2)));
        // Classes from rdf:type objects, desc: Person(2) before City(1).
        assert_eq!(
            card.classes,
            vec![
                ("<http://ex/Person>".to_string(), 2),
                ("<http://ex/City>".to_string(), 1)
            ]
        );
        // Vocabularies: ex/ namespace + the rdf-syntax-ns# namespace.
        assert!(card.vocabularies.contains(&"http://ex/".to_string()));
        assert!(card
            .vocabularies
            .iter()
            .any(|v| v.ends_with("rdf-syntax-ns#")));
    }

    #[test]
    fn named_graph_triples_excluded_from_default_stats() {
        let quads = vec![
            q("<http://ex/a>", "<http://ex/p>", "<http://ex/b>"),
            (
                "<http://ex/c>".into(),
                "<http://ex/p>".into(),
                "<http://ex/d>".into(),
                Some("<http://ex/g1>".into()),
            ),
        ];
        let card = derive_card(&quads, 5, 1, CardInput::default());
        assert_eq!(card.triple_count, 1, "only the default-graph triple counts");
        assert_eq!(card.quad_count, 2);
        assert_eq!(card.named_graph_count, 1);
        assert_eq!(card.predicates[0].1, 1);
    }

    /// A literal-bearing triple helper for the enriched-profile tests.
    const LABEL: &str = "<http://www.w3.org/2000/01/rdf-schema#label>";
    const SAMEAS: &str = "<http://www.w3.org/2002/07/owl#sameAs>";
    const XSD_INT: &str = "http://www.w3.org/2001/XMLSchema#integer";
    const XSD_GYEAR: &str = "http://www.w3.org/2001/XMLSchema#gYear";

    fn enriched_fixture() -> Vec<(String, String, String, Option<String>)> {
        vec![
            q("<http://ex/a>", TYPE, "<http://ex/Person>"),
            q("<http://ex/b>", TYPE, "<http://ex/Person>"),
            q("<http://ex/c>", TYPE, "<http://ex/City>"),
            q("<http://ex/a>", LABEL, "\"Alice\"@en"),
            q("<http://ex/b>", LABEL, "\"Bob\"@en"),
            q(
                "<http://ex/a>",
                "<http://ex/age>",
                &format!("\"30\"^^<{XSD_INT}>"),
            ),
            q(
                "<http://ex/a>",
                "<http://ex/birthYear>",
                &format!("\"1850\"^^<{XSD_GYEAR}>"),
            ),
            q(
                "<http://ex/b>",
                "<http://ex/birthYear>",
                &format!("\"1875\"^^<{XSD_GYEAR}>"),
            ),
            q("<http://ex/a>", "<http://ex/knows>", "<http://ex/b>"),
            q("<http://ex/a>", "<http://ex/livesIn>", "<http://ex/c>"),
            q("<http://ex/a>", SAMEAS, "<http://other/a>"),
        ]
    }

    #[test]
    fn enriched_profile_is_populated() {
        let quads = enriched_fixture();
        let card = derive_card(&quads, 12, 0, CardInput::default());

        assert!(!card.datatypes.is_empty(), "datatypes derived");
        assert!(!card.languages.is_empty(), "languages derived");
        assert!(!card.class_links.is_empty(), "class_links derived");
        assert!(!card.top_hubs.is_empty(), "out-degree hubs derived");
        assert!(!card.in_hubs.is_empty(), "in-degree hubs derived");

        // Effective schema: Person --knows--> Person, Person --livesIn--> City.
        assert!(card
            .class_links
            .iter()
            .any(|l| l.s_class == "<http://ex/Person>"
                && l.predicate == "<http://ex/knows>"
                && l.o_class == "<http://ex/Person>"
                && l.count == 1));
        assert!(card
            .class_links
            .iter()
            .any(|l| l.o_class == "<http://ex/City>"));
        // Untyped external object is the `(untyped)` sentinel.
        assert!(card.class_links.iter().any(|l| l.o_class == "(untyped)"));

        // Datatypes: integer, gYear, and langString from the @en labels.
        assert!(card.datatypes.iter().any(|(d, _)| d.contains("integer")));
        assert!(card.datatypes.iter().any(|(d, _)| d.contains("gYear")));
        assert!(card.datatypes.iter().any(|(d, _)| d.contains("langString")));
        // Languages: "en" present, plus untagged "".
        assert!(card.languages.iter().any(|(l, _)| l == "en"));

        // Signals.
        let s = &card.signals;
        assert_eq!(s.label_predicate.as_deref(), Some(LABEL));
        assert_eq!(s.default_lang.as_deref(), Some("en"));
        assert_eq!(s.base_iri.as_deref(), Some("http://ex/"));
        assert!(s.link_predicates.iter().any(|p| p.contains("sameAs")));
        assert!(s.time_predicates.iter().any(|p| p.contains("birthYear")));
        assert_eq!(
            s.temporal_extent,
            Some(("1850".to_string(), "1875".to_string())),
            "extent over the dominant time predicate"
        );
        // No geometry in this fixture.
        assert!(!s.geo_wkt && !s.geo_latlong);
        assert!(s.spatial_bbox.is_none());
    }

    #[test]
    fn signals_detect_geometry() {
        let wkt = "http://www.opengis.net/ont/geosparql#wktLiteral";
        let quads = vec![
            q("<http://ex/p>", TYPE, "<http://ex/Place>"),
            q(
                "<http://ex/p>",
                "<http://www.w3.org/2003/01/geo/wgs84_pos#lat>",
                "\"41.9\"^^<http://www.w3.org/2001/XMLSchema#double>",
            ),
            q(
                "<http://ex/p>",
                "<http://www.w3.org/2003/01/geo/wgs84_pos#long>",
                "\"12.5\"^^<http://www.w3.org/2001/XMLSchema#double>",
            ),
            q(
                "<http://ex/p>",
                "<http://www.opengis.net/ont/geosparql#asWKT>",
                &format!("\"POINT(12.5 41.9)\"^^<{wkt}>"),
            ),
        ];
        let card = derive_card(&quads, 6, 0, CardInput::default());
        assert!(card.signals.geo_wkt, "asWKT/wktLiteral detected");
        assert!(card.signals.geo_latlong, "wgs84 lat+long detected");
        let bbox = card.signals.spatial_bbox.expect("bbox derived");
        assert_eq!(
            bbox,
            [12.5, 41.9, 12.5, 41.9],
            "[minLon,minLat,maxLon,maxLat]"
        );
    }

    #[test]
    fn class_links_match_schema_summary() {
        use rete_core::{ingest, schema_summary, Rete};
        let quads = enriched_fixture();
        let card = derive_card(&quads, 12, 0, CardInput::default());

        // Build the same data and compute the reference quotient from the index.
        let (bytes, _) =
            ingest::assemble_dataset_with_opts(quads, true, false, None, |_, _| Vec::new());
        let rete = Rete::open(&bytes).unwrap();
        let reference: BTreeSet<(String, String, String, u64)> = schema_summary(&rete)
            .into_iter()
            .map(|(s, p, o, c)| (s, p, o, c as u64))
            .collect();

        let derived: BTreeSet<(String, String, String, u64)> = card
            .class_links
            .iter()
            .map(|l| {
                (
                    l.s_class.clone(),
                    l.predicate.clone(),
                    l.o_class.clone(),
                    l.count,
                )
            })
            .collect();

        assert_eq!(
            derived, reference,
            "card class_links must equal schema_summary's quotient row-for-row"
        );
    }

    #[test]
    fn derive_is_deterministic() {
        // The card folds into the content hash, so the same input must yield
        // byte-identical JSON across rebuilds (no map-iteration nondeterminism).
        let quads = enriched_fixture();
        let a = derive_card(&quads, 12, 0, CardInput::default()).to_json_bytes();
        let b = derive_card(&quads, 12, 0, CardInput::default()).to_json_bytes();
        assert_eq!(a, b, "enriched card derivation must be deterministic");
        assert!(!a.is_empty());
    }

    #[test]
    fn old_plain_string_card_still_parses() {
        // A pre-enrichment card: example_queries as plain strings, none of the
        // new fields present. Serde `default`s must keep it deserializing.
        let json = r#"{
            "title":"Legacy",
            "triple_count":3,"quad_count":3,"named_graph_count":0,"term_count":5,
            "predicates":[["<http://ex/p>",2]],
            "example_queries":["SELECT * WHERE { ?s ?p ?o }"],
            "format_version":1
        }"#;
        let card = DatasetCard::from_json_bytes(json.as_bytes()).unwrap();
        assert_eq!(card.title.as_deref(), Some("Legacy"));
        assert_eq!(card.example_queries.len(), 1);
        // Every enriched field defaults to empty/false when absent from the JSON.
        assert!(card.datatypes.is_empty());
        assert!(card.languages.is_empty());
        assert!(card.class_links.is_empty());
        assert!(card.top_hubs.is_empty());
        assert!(card.in_hubs.is_empty());
        assert!(card.queries.is_empty());
        assert!(card.signals.is_empty());
        assert!(!card.truncated);
    }

    #[test]
    fn coherence_stamp_is_deterministic_and_additive() {
        let dis = "<http://www.w3.org/2002/07/owl#disjointWith>";
        let rt = "<http://www.w3.org/1999/02/22-rdf-syntax-ns#type>";
        let base = vec![
            ("<http://ex/C>".into(), dis.into(), "<http://ex/D>".into()),
            ("<http://ex/x>".into(), rt.into(), "<http://ex/C>".into()),
            ("<http://ex/x>".into(), rt.into(), "<http://ex/D>".into()),
        ];
        let r = rete_core::reason(&base);
        let c1 = Coherence::from_reasoning(&r, false);
        // Deterministic + the right verdict (no free-text detail in the stamp).
        assert_eq!(c1, Coherence::from_reasoning(&r, false));
        assert!(!c1.coherent);
        assert_eq!(c1.inconsistency_count, 1);
        assert_eq!(c1.by_kind, vec![("disjoint-classes".to_string(), 1)]);
        assert_eq!(c1.rules, rete_core::REASON_RULESET);

        // A stamped card round-trips; an unstamped one omits the block entirely.
        let json = DatasetCard::default()
            .with_coherence(&r, false)
            .to_json_bytes();
        let text = std::str::from_utf8(&json).unwrap();
        assert!(text.contains("coherence"));
        assert_eq!(DatasetCard::from_json_bytes(&json).unwrap().coherence, c1);

        let plain = DatasetCard::default().to_json_bytes();
        assert!(!std::str::from_utf8(&plain).unwrap().contains("coherence"));
        assert!(DatasetCard::default().coherence.is_empty());
    }

    #[test]
    fn base_iri_is_common_prefix_with_fallback() {
        // Subjects under several sibling namespaces → their common root.
        let mut ns = BTreeMap::new();
        ns.insert("http://ex/person/".to_string(), 2000u64);
        ns.insert("http://ex/place/".to_string(), 600);
        ns.insert("http://ex/org/".to_string(), 120);
        assert_eq!(derive_base_iri(&ns).as_deref(), Some("http://ex/"));

        // A single namespace is its own base.
        let mut one = BTreeMap::new();
        one.insert("http://data.example.org/id/".to_string(), 10u64);
        assert_eq!(
            derive_base_iri(&one).as_deref(),
            Some("http://data.example.org/id/")
        );

        // Subjects across unrelated hosts: the LCP collapses to `http://`, so we
        // fall back to the dominant namespace rather than ship a useless prefix.
        let mut multi = BTreeMap::new();
        multi.insert("http://a.example/".to_string(), 100u64);
        multi.insert("http://b.other/".to_string(), 5);
        assert_eq!(
            derive_base_iri(&multi).as_deref(),
            Some("http://a.example/")
        );

        assert_eq!(derive_base_iri(&BTreeMap::new()), None);
    }

    #[test]
    fn parse_literal_decomposes_forms() {
        let plain = parse_literal("\"hello\"").unwrap();
        assert_eq!(plain.value, "hello");
        assert!(plain.datatype.ends_with("XMLSchema#string"));
        assert_eq!(plain.lang, "");

        let tagged = parse_literal("\"bonjour\"@fr").unwrap();
        assert_eq!(tagged.lang, "fr");
        assert!(tagged.datatype.ends_with("#langString"));

        let typed = parse_literal("\"42\"^^<http://www.w3.org/2001/XMLSchema#integer>").unwrap();
        assert_eq!(typed.value, "42");
        assert_eq!(typed.datatype, "http://www.w3.org/2001/XMLSchema#integer");

        // Escaped quote inside the value is not mistaken for the terminator.
        let esc = parse_literal("\"a\\\"b\"@en").unwrap();
        assert_eq!(esc.value, "a\\\"b");
        assert_eq!(esc.lang, "en");

        // Non-literals decline.
        assert!(parse_literal("<http://ex/x>").is_none());
        assert!(parse_literal("_:b0").is_none());
    }

    #[test]
    fn flags_override_card_file_fields() {
        // No file, just flags → flags populate curated fields.
        let args = CardArgs {
            title: Some("Flag Title".into()),
            license: Some("MIT".into()),
            ..Default::default()
        };
        assert!(args.requested());
        let curated = load_curated(&args).unwrap();
        assert_eq!(curated.title.as_deref(), Some("Flag Title"));
        assert_eq!(curated.license.as_deref(), Some("MIT"));

        // No flags at all → not requested.
        assert!(!CardArgs::default().requested());
    }

    #[test]
    fn hex16_formats_lowercase_padded() {
        let mut b = [0u8; 16];
        b[0] = 0x0a;
        b[15] = 0xff;
        let h = hex16(&b);
        assert_eq!(h.len(), 32);
        assert!(h.starts_with("0a"));
        assert!(h.ends_with("ff"));
    }
}
