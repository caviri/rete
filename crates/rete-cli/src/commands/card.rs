//! Dataset Cards — an embeddable data-catalog record stored in a `.rete` file's
//! metadata section. A card carries **curated** metadata (title, license,
//! source, description, created, keywords/theme, example queries, plus a
//! bounded bag of publisher-defined custom fields under `extra`) and
//! **auto-derived**
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

    // --- Curated identity & provenance (issue #153). All deterministic
    // (hand-supplied via `--card-file`), so they live INSIDE the content hash;
    // the per-build volatile facts (timestamp, builder, timings) live in the
    // separate unhashed build-info section instead. ---
    /// The publisher's **dataset version** (e.g. a date or semver) — Croissant
    /// requires one. One of three distinct versions a `.rete` carries, each
    /// with its own owner, never merged: `version` (the *data*, set by the
    /// publisher here), `format_version` (the *spec* the file conforms to,
    /// stamped by the builder), and the builder's own identity
    /// (`rete-cli 0.3.2`, in the build-info section).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    /// The people who made the dataset. ORCID as an IRI, not a string, so a
    /// card's creator is joinable against the published ORCID graph.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub creators: Vec<Creator>,
    /// Publishing organisation, with its ROR IRI for the same joinability.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub publisher: Option<Publisher>,
    /// Where the authoritative copy of THIS file lives (`void:dataDump`). A
    /// `.rete` found on a disk can then say where to verify against.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub canonical_url: Option<String>,
    /// A public SPARQL endpoint serving this dataset (`void:sparqlEndpoint`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sparql_endpoint: Option<String>,
    /// The SOURCE data's own date (harvest/snapshot), distinct from both the
    /// curated `created` and the build-info timestamp.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_date: Option<String>,
    /// What this file was derived from (`prov:wasDerivedFrom`): source dumps,
    /// upstream `.rete` shards a merge folded in, an endpoint harvested.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub derived_from: Vec<String>,
    /// DOI of the dataset, as an IRI (`https://doi.org/…`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub doi: Option<String>,
    /// Preferred citation text.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cite_as: Option<String>,
    /// Free-text keywords/tags describing the dataset. First-class rather
    /// than an `extra` entry because keywords carry **agreed meaning**:
    /// `dcat:keyword` and `schema:keywords` are long-standing terms (DCAT-AP
    /// catalogs require the former; dataset-search harvesters read the
    /// latter), and the bag's contract is precisely the *absence* of a term —
    /// it projects as opaque values. Canonicalized by
    /// [`normalize_string_list`] at build time: trimmed, sorted, deduplicated
    /// — `dcat:keyword` is an unordered repeated property, so sorting loses
    /// nothing and keeps the card's bytes (hence the reproducible content
    /// hash) independent of authoring order.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub keywords: Vec<String>,
    /// Curated **themes** — IRIs into a controlled vocabulary
    /// (`dcat:theme`, e.g. the EU data-theme authority). IRIs are
    /// **required** ([`normalize_themes`]): a free-text theme is a keyword by
    /// another name and belongs in `keywords` — the IRI into an agreed
    /// scheme is exactly what makes `dcat:theme` worth a separate field.
    /// Sorted/deduplicated like `keywords`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub theme: Vec<String>,
    // There is deliberately NO curated language field next to these: in RDF
    // the language rides on each literal (`@lang` / `rdf:langString`), so the
    // dataset's languages are DERIVABLE — and derived below (`languages`,
    // `signals.default_lang`). A curated duplicate would be a second source
    // of truth that can drift from the data with no way to tell which is
    // right. See "First-class field or the bag?" in docs/dataset-cards.md.
    /// Publisher-defined **custom fields** — one bounded, reserved bag.
    /// Anything under `extra` is by definition *not* a rete-defined field:
    /// official card fields are only ever added at the top level (which
    /// [`CardInput`] keeps reserved by rejecting unknown keys), so a future
    /// release can never collide with a publisher's key. Curated input, so
    /// the bag folds into the content hash; [`normalize_extra`] canonicalizes
    /// (sorts) nested object keys and enforces the `CARD_EXTRA_*` bounds at
    /// build time. Readers accept whatever is present — the limits bite when
    /// writing, never when reading.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub extra: BTreeMap<String, serde_json::Value>,

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
    /// The top-N cap the profile lists were derived under — the number
    /// `truncated: true` was hinting at without stating. 0 (omitted) on cards
    /// whose profile was never derived (external builds, pre-existing cards).
    #[serde(default, skip_serializing_if = "is_zero_u32")]
    pub top_n: u32,

    /// The `.rete` **spec version** the file was written against (currently
    /// 5). The format's version — not the dataset's (the curated `version`
    /// above) and not the builder's (`builder` in build-info).
    pub format_version: u8,
}

/// serde helper: omit a `0` cap (profile-less cards).
fn is_zero_u32(v: &u32) -> bool {
    *v == 0
}

/// A dataset creator: a person (or team) with an optional ORCID IRI.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Creator {
    pub name: String,
    /// `https://orcid.org/0000-0000-0000-0000`
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub orcid: Option<String>,
}

/// The publishing organisation, with an optional ROR IRI.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Publisher {
    pub name: String,
    /// `https://ror.org/…`
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ror: Option<String>,
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
    /// Whether the file carries a **full-text (TEXT_INDEX) section**.
    ///
    /// Unlike every other signal this one is **not derived from the triples and
    /// not stored in the card**: it is measured from the file's section
    /// directory by whoever reads the card, and [`DatasetCard::to_json_bytes`]
    /// strips it before the bytes are written. See [`TextIndexSignal`] for why.
    ///
    /// `None` therefore means **unknown** — a card read out of a saved JSON
    /// document, with no file to measure — and never "no index".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text_index: Option<TextIndexSignal>,
}

impl Signals {
    /// True when no affordance was detected (lets the whole block be omitted).
    fn is_empty(&self) -> bool {
        *self == Signals::default()
    }
}

/// The file's full-text-search affordance: does it carry a **TEXT_INDEX section
/// (kind 6)**, and what does that index cost?
///
/// A `.rete` built with `--text-index` answers `FILTER(CONTAINS(…))` by word
/// lookup; one built without it answers the *same query with the same rows* by
/// full scan. The capability is invisible from the results, which is exactly how
/// the playground catalog came to advertise an index two published files never
/// carried (#189) — so a file has to be able to state it.
///
/// **Measured at read time, never stored.** The ground truth is the section
/// directory in the 1 KiB header that every card read already fetches, so a
/// stored copy would be a second source of truth for a fact the first source
/// answers for free — the same reasoning that keeps a curated `language` field
/// out of the card. Being a projection rather than a stamp also means every
/// already-published file reports it today, with no re-card, and that
/// `rete repyramid --text-index` (which rewrites the file's sections) can never
/// leave it stale.
///
/// **What it deliberately does not carry: a token count.** The number of
/// distinct indexed words is the first varint of the *decompressed* token
/// table, so quoting it would mean fetching and inflating the whole table —
/// 193 MB on the published `causenet-full-typed.rete`. [`token_table_bytes`] is
/// the same question answered for ≤10 bytes, and is the more useful answer
/// anyway: it is what a first search actually costs.
///
/// [`token_table_bytes`]: TextIndexSignal::token_table_bytes
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct TextIndexSignal {
    /// True iff the file has a kind-6 section. Always written, including as
    /// `false`: "measured, and there is none" is the fact #189 was missing.
    pub present: bool,
    /// Byte length of the whole TEXT_INDEX section — free, straight from the
    /// header. Worth stating because it can dominate the download:
    /// `causenet-full-typed.rete` carries 1.88 GB of it, 29% of the file.
    #[serde(default, skip_serializing_if = "is_zero_u64")]
    pub bytes: u64,
    /// Byte length of the section's leading **token table** — the prefix a
    /// first search faults, several times smaller than `bytes` (which counts
    /// the postings blob, fetched one posting at a time). One ≤10-byte range
    /// read; `None` when there is no index, or its first bytes were unreadable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token_table_bytes: Option<u64>,
}

/// serde helper: omit a `0` byte count (no section to measure).
fn is_zero_u64(v: &u64) -> bool {
    *v == 0
}

impl TextIndexSignal {
    /// Measure the signal from a file's header, through the same
    /// [`rete_core::RangeReader`] the card was read with. Costs nothing beyond
    /// the header already in hand when there is no index, and one ≤10-byte
    /// range read when there is — never the index itself.
    pub(crate) fn probe<R: rete_core::RangeReader + ?Sized>(reader: &R, header: &Header) -> Self {
        if header.text_index_len == 0 {
            return TextIndexSignal::default();
        }
        TextIndexSignal {
            present: true,
            bytes: header.text_index_len,
            token_table_bytes: rete_core::read_text_index_token_table_len_ranged(reader, header),
        }
    }

    /// One line for the human catalog view and the audit report.
    pub(crate) fn describe(&self) -> String {
        if !self.present {
            return "no TEXT_INDEX section — CONTAINS/regex still answer, by full scan".to_string();
        }
        match self.token_table_bytes {
            Some(tt) => format!(
                "TEXT_INDEX present — {} bytes ({tt} of them the token table a first search reads)",
                self.bytes
            ),
            None => format!("TEXT_INDEX present — {} bytes", self.bytes),
        }
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
/// optional). CLI flags override whatever the file provides. The identity/
/// provenance fields (version, creators, publisher, canonical_url,
/// sparql_endpoint, source_date, derived_from, doi, cite_as, keywords, theme)
/// have no CLI flag — they come from the card file only.
///
/// `deny_unknown_fields` keeps the card file's top level **reserved for
/// rete-defined fields**: a stray key is a loud error (usually a typo, or a
/// custom field that belongs inside `extra`), never a silent drop — and it is
/// what makes the collision guarantee real: a publisher's field can only ever
/// live in the bag, so a future official top-level field cannot capture one.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct CardInput {
    pub title: Option<String>,
    /// A string, or an array of lines joined with `\n` — see
    /// [`de_description`]. Markdown is allowed here (raw HTML is not); the
    /// array shape exists because hand-writing `\n` escapes in JSON is awful.
    #[serde(default, deserialize_with = "de_description")]
    pub description: Option<String>,
    pub license: Option<String>,
    pub source: Option<String>,
    pub created: Option<String>,
    pub version: Option<String>,
    #[serde(default)]
    pub creators: Vec<Creator>,
    pub publisher: Option<Publisher>,
    pub canonical_url: Option<String>,
    pub sparql_endpoint: Option<String>,
    pub source_date: Option<String>,
    #[serde(default)]
    pub derived_from: Vec<String>,
    pub doi: Option<String>,
    pub cite_as: Option<String>,
    /// Free-text keywords (see [`DatasetCard::keywords`]).
    #[serde(default)]
    pub keywords: Vec<String>,
    /// Controlled-vocabulary theme IRIs (see [`DatasetCard::theme`]).
    #[serde(default)]
    pub theme: Vec<String>,
    /// Publisher-defined custom fields (see [`DatasetCard::extra`]).
    #[serde(default)]
    pub extra: BTreeMap<String, serde_json::Value>,
    #[serde(default)]
    pub example_queries: Vec<String>,
}

/// Deserialize `description` from either a JSON string or an array of lines.
///
/// A description may be Markdown, and Markdown needs line breaks; in a JSON
/// string those are `\n` escapes, which are miserable to write by hand and
/// worse to review in a diff. An array of lines — joined with `\n` — reads as
/// the Markdown it is. It is **input sugar only**: the card stores one string
/// either way, so `rete card --json` output feeds straight back into
/// `--card-file`. The shared rule lives in `rete_core::card` so the browser
/// builder accepts exactly the same two shapes.
fn de_description<'de, D>(d: D) -> Result<Option<String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    use serde::de::Error as _;
    let v = Option::<serde_json::Value>::deserialize(d)?;
    match v {
        None | Some(serde_json::Value::Null) => Ok(None),
        Some(v) => rete_core::card::normalize_description(&v)
            .map(Some)
            .map_err(D::Error::custom),
    }
}

impl CardInput {
    /// Move every curated field into a card (the shared step of all derivation
    /// paths, so a new curated field is wired exactly once).
    fn fill(self, card: &mut DatasetCard) {
        card.title = self.title;
        card.description = self.description;
        card.license = self.license;
        card.source = self.source;
        card.created = self.created;
        card.version = self.version;
        card.creators = self.creators;
        card.publisher = self.publisher;
        card.canonical_url = self.canonical_url;
        card.sparql_endpoint = self.sparql_endpoint;
        card.source_date = self.source_date;
        card.derived_from = self.derived_from;
        card.doi = self.doi;
        card.cite_as = self.cite_as;
        card.keywords = self.keywords;
        card.theme = self.theme;
        card.extra = self.extra;
        card.example_queries = self.example_queries;
    }
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
    ///
    /// [`Signals::text_index`] is **stripped** on the way out: it is measured
    /// from the file's section directory at read time, and a copy inside the
    /// hashed metadata section would be a claim that can outlive the sections it
    /// describes (`rete repyramid --text-index` rewrites them). Stripping here —
    /// the single choke point every card-writing command funnels through — is
    /// what makes "derived, never authored" true of the bytes and not only of
    /// the intent. (`CardInput` has no `signals` field and rejects unknown keys,
    /// so a hand-written card file cannot set it either.)
    pub(crate) fn to_json_bytes(&self) -> Vec<u8> {
        if self.signals.text_index.is_none() {
            return serde_json::to_vec(self).expect("DatasetCard serializes");
        }
        let mut stored = self.clone();
        stored.signals.text_index = None;
        serde_json::to_vec(&stored).expect("DatasetCard serializes")
    }

    /// Attach the read-time [`TextIndexSignal`], returning whatever the card's
    /// own bytes claimed before it (normally `None` — the writers strip it).
    ///
    /// A `Some(_)` return is drift: a card asserting a full-text index that no
    /// longer has to be believed, because the file itself was just measured.
    /// `rete card-audit` reports it.
    pub(crate) fn observe_text_index(
        &mut self,
        measured: TextIndexSignal,
    ) -> Option<TextIndexSignal> {
        self.signals.text_index.replace(measured)
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

    /// Replace the derived sizes with the counts the file was actually written
    /// with (consuming builder).
    ///
    /// A card is derived from the ingested statements, which is the input
    /// multiset; the file holds that multiset **deduplicated**, because every
    /// permutation index sorts and dedups. The two agree for input with no
    /// duplicates — and only then, which is why `rete card` used to report the
    /// raw harvest size of any dataset paged with overlapping windows while
    /// `rete info` reported the real one. The header's count is authoritative;
    /// this is how it reaches the card. See `rete_core::FinalCounts`.
    ///
    /// The distributions (predicate/class histograms, hub degrees) stay over the
    /// derived multiset — they are shapes, not sizes.
    pub(crate) fn with_final_counts(mut self, counts: rete_core::ingest::FinalCounts) -> Self {
        self.triple_count = counts.default_triples;
        self.quad_count = counts.quads;
        self
    }
}

/// The versioned JSON envelope used by the local and remote `card --json`
/// commands. Embedded metadata remains backward-readable; the CLI contract adds
/// its schema version at presentation time.
pub(crate) fn card_json(card: &DatasetCard) -> serde_json::Value {
    let mut value = serde_json::to_value(card).expect("DatasetCard serializes");
    value
        .as_object_mut()
        .expect("DatasetCard JSON is an object")
        .insert(
            "schemaVersion".into(),
            serde_json::json!(crate::JSON_SCHEMA_VERSION),
        );
    value
}

/// [`card_json`] plus the build-info record (when the file carries one) under a
/// `"build"` key — an envelope addition, so the card's own stored bytes stay
/// exactly the hashed metadata section.
pub(crate) fn card_json_with_build(
    card: &DatasetCard,
    build: Option<&super::buildinfo::BuildInfo>,
) -> serde_json::Value {
    let mut value = card_json(card);
    if let Some(b) = build {
        value
            .as_object_mut()
            .expect("DatasetCard JSON is an object")
            .insert(
                "build".into(),
                serde_json::to_value(b).expect("BuildInfo serializes"),
            );
    }
    value
}

/// Resolve the curated fields: load `--card-file` (if any), then let explicit
/// flags override individual fields. Custom fields have **no flag** — arbitrary
/// key/values on a command line are a shell-quoting trap with no schema to
/// validate against; they come from the card file's `extra` object only, and
/// the bag is normalized and bounds-checked here, the single choke point every
/// card-writing command (`build`, `merge`, `repyramid`) funnels through.
pub(crate) fn load_curated(args: &CardArgs) -> anyhow::Result<CardInput> {
    let mut c = match &args.file {
        Some(path) => {
            let text = std::fs::read_to_string(path)
                .map_err(|e| anyhow::anyhow!("reading --card-file {path}: {e}"))?;
            serde_json::from_str(&text).map_err(|e| {
                // The top level is reserved for rete-defined fields; point a
                // stray key at the bag instead of leaving a bare serde error.
                let hint = if e.to_string().contains("unknown field") {
                    "\n  (the card file's top level is reserved for rete-defined fields; \
                     publisher-defined fields go inside the \"extra\" object — \
                     see docs/dataset-cards.md)"
                } else {
                    ""
                };
                anyhow::anyhow!("parsing --card-file {path}: {e}{hint}")
            })?
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
    // After the override, so `--description` is bounded exactly like a
    // `--card-file` one. (`--description "$(cat desc.md)"` is the shell-side
    // answer to authoring a multi-line description — see docs/dataset-cards.md.)
    if let Some(d) = &c.description {
        rete_core::card::check_description_len(d).map_err(|e: String| anyhow::anyhow!(e))?;
    }
    c.keywords = normalize_string_list("keywords", std::mem::take(&mut c.keywords))?;
    c.theme = normalize_themes(std::mem::take(&mut c.theme))?;
    c.extra = normalize_extra(std::mem::take(&mut c.extra))?;
    Ok(c)
}

/// Canonicalize a curated string list (`keywords`, `theme`) — the write-time
/// gate every card-writing command funnels through (like [`normalize_extra`]).
///
/// The rules themselves live in [`rete_core::card::normalize_string_list`]:
/// the browser builder writes cards too, and one implementation is the only
/// way `rete build --card-file` and the playground can be guaranteed to reject
/// the same documents with the same words. This is the `anyhow` face of it.
pub(crate) fn normalize_string_list(
    field: &str,
    values: Vec<String>,
) -> anyhow::Result<Vec<String>> {
    rete_core::card::normalize_string_list(field, values).map_err(|e| anyhow::anyhow!(e))
}

/// [`normalize_string_list`] plus the `theme` requirement: every entry must
/// be an **IRI into a controlled vocabulary**. This is what keeps `theme` from
/// becoming a second keywords field — a free-text theme carries no more
/// meaning than a keyword, and `keywords` already holds those; the agreed
/// concept scheme behind the IRI is the whole value `dcat:theme` adds.
/// (Shared implementation: [`rete_core::card::normalize_themes`].)
pub(crate) fn normalize_themes(themes: Vec<String>) -> anyhow::Result<Vec<String>> {
    rete_core::card::normalize_themes(themes).map_err(|e| anyhow::anyhow!(e))
}

/// Cap for every top-N list embedded in the card. The metadata section is
/// fetched on **every** overview (it is part of the index-free CARD tier), so an
/// unbounded `class_links` (O(classes × predicates × classes)) or predicate list
/// would bloat that fetch on a large schema (CIDOC-CRM/MMM). Capping keeps the
/// card small and bounded; `truncated` flags when a list was actually cut.
pub(crate) const CARD_TOP_N: usize = 100;

// The `extra` bag's bounds are format-level facts, not CLI policy — every
// writer has to honour them — so they live in `rete-core` beside the validator
// that enforces them (`rete_core::card::CARD_EXTRA_*`).

/// Canonicalize and bounds-check the `extra` bag — the write-time gate.
///
/// On overflow the build is **rejected loudly** rather than truncated quietly:
/// `extra` is authored (unlike the derived lists, which are capped with
/// `truncated` set, because they can be re-derived), so cutting it would
/// silently ship a card that no longer says what the publisher wrote — and,
/// since the bag folds into the content hash, "what I wrote" and "what hashed"
/// would diverge invisibly. Only the author can decide what to trim.
/// (Shared implementation: [`rete_core::card::normalize_extra`]; the card
/// carries the bag as a `BTreeMap` for stable serde ordering, so this converts
/// at the boundary.)
pub(crate) fn normalize_extra(
    extra: BTreeMap<String, serde_json::Value>,
) -> anyhow::Result<BTreeMap<String, serde_json::Value>> {
    let map: serde_json::Map<String, serde_json::Value> = extra.into_iter().collect();
    let checked = rete_core::card::normalize_extra(map).map_err(|e| anyhow::anyhow!(e))?;
    Ok(checked.into_iter().collect())
}

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
// `pub(crate)`: the query generator gates its geometry template on which of
// these two the card actually recorded, so both modules must name the same IRI.
pub(crate) const GEO_ASWKT: &str = "<http://www.opengis.net/ont/geosparql#asWKT>";
pub(crate) const GEO_HASGEOMETRY: &str = "<http://www.opengis.net/ont/geosparql#hasGeometry>";
// Datatype IRIs (unbracketed — as they appear after `^^` in a literal term).
const GEO_WKTLITERAL: &str = "http://www.opengis.net/ont/geosparql#wktLiteral";
const XSD_STRING: &str = "http://www.w3.org/2001/XMLSchema#string";
const RDF_LANGSTRING: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#langString";

/// The [`ClassLink::o_class`] sentinel for a literal object (the object of that
/// row is a literal, so it has no class and no outgoing edges). `pub(crate)`:
/// the query generator reads it to tell an entity-to-entity relation — one a
/// path query can walk — from a literal-valued one.
pub(crate) const O_LITERAL: &str = "(literal)";

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
/// A card holding curated fields + top-line counts only — for the external
/// (memory-bounded) build, where deriving the profile lists (predicates/classes/
/// hubs/links) would need unbounded RAM. Every derived list is left empty and
/// `truncated` is unset (the lists are absent, not cut).
pub(crate) fn curated_counts_card(
    statements: u64,
    term_count: u64,
    curated: CardInput,
) -> DatasetCard {
    let mut card = DatasetCard {
        triple_count: statements,
        quad_count: statements,
        named_graph_count: 0,
        term_count,
        format_version: rete_core::format::CURRENT_FORMAT_VERSION,
        ..DatasetCard::default()
    };
    curated.fill(&mut card);
    card
}

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
            O_LITERAL.to_string()
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
        // Measured by the READER from the file's section directory, never
        // derived from the triples and never written — see `TextIndexSignal`.
        text_index: None,
    };

    let mut card = DatasetCard {
        triple_count,
        quad_count,
        named_graph_count,
        term_count,
        predicates,
        classes,
        vocabularies,
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
        // The cap the profile lists above were derived under (deterministic:
        // a compile-time constant of this builder, not a per-build fact).
        top_n: CARD_TOP_N as u32,
        format_version: rete_core::CURRENT_FORMAT_VERSION,
        ..DatasetCard::default()
    };
    curated.fill(&mut card);
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

/// One CARD-tier read of a `.rete`: everything a reader can learn about the
/// file without touching the dictionary, index or pyramid.
pub(crate) struct CardRead {
    /// The 1 KiB header, parsed once and reused (checksum, section directory).
    pub header: Header,
    /// The embedded card, with [`Signals::text_index`] already measured.
    pub card: Option<DatasetCard>,
    /// The adjacent build-info record, when the file carries one.
    pub build: Option<super::buildinfo::BuildInfo>,
    /// What the header says about the file's full-text index — measured, so it
    /// is an answer even for a file with no card at all.
    pub text_index: TextIndexSignal,
    /// What the card's own bytes claimed about the index before the measurement
    /// replaced it. Normally `None`; `Some(_)` is drift worth reporting.
    pub stored_text_index: Option<TextIndexSignal>,
}

/// Read the header, the card, **and** the build-info record in the CARD tier's
/// budget: the 1 KiB header read plus one coalesced range covering both
/// adjacent sections (the same adjacency contract as
/// [`rete_core::read_card_and_build_info_ranged`], done here so the header is
/// fetched exactly once and reused for the checksum). A build-info blob that
/// fails to parse degrades to `None` with a warning — a newer writer's record
/// must never make the card unreadable.
///
/// The header this already holds also answers whether the file carries a
/// full-text index, so the [`TextIndexSignal`] is measured here — the one place
/// every card-reading command funnels through — rather than in each of them. It
/// adds one ≤10-byte range read, and only when there is an index to measure.
pub(crate) fn load_card_and_build_ranged<R: rete_core::RangeReader>(
    reader: &R,
) -> anyhow::Result<CardRead> {
    let head = reader.read_at(0, rete_core::HEADER_LEN as u64)?;
    let header = Header::from_bytes(&head)?;
    let meta = (header.metadata_offset, header.metadata_len);
    let build = (header.build_info_offset, header.build_info_len);
    let (m, b) = if meta.1 > 0 && build.1 > 0 && build.0 == meta.0 + meta.1 {
        // Adjacent (the layout the writers produce): one read spans both.
        let both = reader.read_at(meta.0, meta.1 + build.1)?;
        let (m, b) = both.split_at(meta.1 as usize);
        (Some(m.to_vec()), Some(b.to_vec()))
    } else {
        let fetch = |off: u64, len: u64| -> std::io::Result<Option<Vec<u8>>> {
            if len == 0 {
                Ok(None)
            } else {
                Ok(Some(reader.read_at(off, len)?))
            }
        };
        (fetch(meta.0, meta.1)?, fetch(build.0, build.1)?)
    };
    let mut card = match m {
        None => None,
        Some(bytes) => Some(DatasetCard::from_json_bytes(&bytes)?),
    };
    let build = b.and_then(
        |bytes| match super::buildinfo::BuildInfo::from_json_bytes(&bytes) {
            Ok(b) => Some(b),
            Err(e) => {
                eprintln!("warning: unreadable build-info section ({e}); ignoring it");
                None
            }
        },
    );
    let text_index = TextIndexSignal::probe(reader, &header);
    let stored_text_index = card
        .as_mut()
        .and_then(|c| c.observe_text_index(text_index))
        .filter(|stored| *stored != text_index);
    Ok(CardRead {
        header,
        card,
        build,
        text_index,
        stored_text_index,
    })
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
    // The description may be Markdown, and Markdown has line breaks. Indent the
    // continuation lines to the value column so a multi-line description stays
    // inside the catalog's layout instead of falling out of it at column 0.
    if let Some(d) = &card.description {
        for (n, line) in d.split('\n').enumerate() {
            if n == 0 {
                let _ = writeln!(out, "  {:<13}: {line}", "description");
            } else {
                let _ = writeln!(out, "  {:<13}  {line}", "");
            }
        }
    }
    field(&mut out, "license", &card.license);
    field(&mut out, "source", &card.source);
    field(&mut out, "created", &card.created);
    if !card.keywords.is_empty() {
        let _ = writeln!(out, "  {:<13}: {}", "keywords", card.keywords.join(", "));
    }
    for t in &card.theme {
        let _ = writeln!(out, "  {:<13}: {t}", "theme");
    }
    field(&mut out, "version", &card.version);
    field(&mut out, "source date", &card.source_date);
    for c in &card.creators {
        let orcid = c
            .orcid
            .as_deref()
            .map(|o| format!("  ({o})"))
            .unwrap_or_default();
        let _ = writeln!(out, "  {:<13}: {}{orcid}", "creator", c.name);
    }
    if let Some(p) = &card.publisher {
        let ror = p
            .ror
            .as_deref()
            .map(|r| format!("  ({r})"))
            .unwrap_or_default();
        let _ = writeln!(out, "  {:<13}: {}{ror}", "publisher", p.name);
    }
    field(&mut out, "canonical URL", &card.canonical_url);
    field(&mut out, "endpoint", &card.sparql_endpoint);
    field(&mut out, "doi", &card.doi);
    field(&mut out, "cite as", &card.cite_as);
    for d in &card.derived_from {
        let _ = writeln!(out, "  {:<13}: {d}", "derived from");
    }
    if !card.extra.is_empty() {
        let _ = writeln!(out, "  custom fields ({}):", card.extra.len());
        for (k, v) in &card.extra {
            // `Value`'s Display is compact JSON — strings keep their quotes,
            // so a value is always unambiguous about its type.
            let _ = writeln!(out, "      {k} = {v}");
        }
    }

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
        // Stated in both directions, and last because it is the one signal
        // measured from the file's sections rather than from its triples. A
        // card read from a saved JSON document has nothing to measure, so it
        // says nothing at all rather than guessing "no".
        if let Some(ti) = &s.text_index {
            let _ = writeln!(out, "      full text  : {}", ti.describe());
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

/// Output shape of `rete card` / `rete card-url`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CardFormat {
    /// Human catalog view (default).
    Text,
    /// The raw card JSON envelope (`--json`).
    Json,
    /// JSON-LD projection: VoID + schema.org + PROV (`--format jsonld`).
    JsonLd,
    /// Croissant projection of the honestly-mappable subset
    /// (`--format croissant`).
    Croissant,
}

impl CardFormat {
    /// Resolve the `--json` / `--format` flags (`--format` wins).
    pub(crate) fn resolve(json: bool, format: Option<&str>) -> anyhow::Result<Self> {
        match format {
            None => Ok(if json {
                CardFormat::Json
            } else {
                CardFormat::Text
            }),
            Some("json") => Ok(CardFormat::Json),
            Some("jsonld") => Ok(CardFormat::JsonLd),
            Some("croissant") => Ok(CardFormat::Croissant),
            Some(other) => anyhow::bail!("unknown card format {other:?} (json|jsonld|croissant)"),
        }
    }
}

/// Shared presentation for `rete card` and `rete card-url`: render one card (+
/// optional build info) in the chosen format. `source` is the file's own
/// path/URL, used as the dataset IRI fallback in the projections.
pub(crate) fn print_card(
    card: &DatasetCard,
    build: Option<&super::buildinfo::BuildInfo>,
    checksum: &str,
    source: &str,
    format: CardFormat,
    sha256: Option<&str>,
) -> anyhow::Result<()> {
    match format {
        CardFormat::Text => {
            println!("{}", format_card(card, checksum));
            if let Some(b) = build {
                println!("{}", super::buildinfo::format_build_info(b));
            }
        }
        CardFormat::Json => println!(
            "{}",
            serde_json::to_string_pretty(&card_json_with_build(card, build))?
        ),
        CardFormat::JsonLd => println!(
            "{}",
            serde_json::to_string_pretty(&super::card_jsonld::to_jsonld(
                card, build, checksum, source
            ))?
        ),
        CardFormat::Croissant => println!(
            "{}",
            serde_json::to_string_pretty(&super::card_jsonld::to_croissant(
                card, checksum, source, sha256
            ))?
        ),
    }
    Ok(())
}

/// `rete card <file> [--json|--format jsonld|croissant]`: print the embedded
/// dataset card (catalog view), the raw JSON, or an RDF projection. Prints
/// `(no dataset card — …)` when absent, still naming what the header alone
/// decides (whether the file carries a full-text index).
pub(crate) fn card_cmd(
    file: &str,
    json: bool,
    format: Option<&str>,
    sha256: Option<&str>,
) -> anyhow::Result<()> {
    let format = CardFormat::resolve(json, format)?;
    // The CARD tier: header + one coalesced metadata+build-info range — the
    // same reads `card-url` does over HTTP, so a 50 GB local file costs KBs to
    // describe.
    let reader = crate::commands::range_source::LocalRangeReader::open(file)?;
    let read = load_card_and_build_ranged(&reader)?;
    match &read.card {
        // A cardless file can still answer the one question the header alone
        // decides — and staying silent about it is what #189 was about.
        None => println!("(no dataset card — {})", read.text_index.describe()),
        Some(card) => print_card(
            card,
            read.build.as_ref(),
            &hex16(&read.header.content_hash),
            file,
            format,
            sha256,
        )?,
    }
    Ok(())
}

/// `rete card-audit` — do the starter queries a card **already ships** still
/// answer on the file that carries them?
///
/// A published `.rete` cannot be re-carded for free (a 250 GB catalog is not a
/// thing to rebuild on a hunch), so this reads the card — two range requests,
/// tens of KB, whatever the file's size — and decides each query's fate from
/// the profile the card carries. The judgement is
/// [`crate::commands::queries::audit`], which shares its one co-occurrence test
/// with the generator itself.
///
/// Input is a `.rete` file (local path or `http(s)://` URL) or a card JSON
/// document as written by `rete card --json` / `rete card-url --json`, so a
/// survey that already fetched the cards need not fetch them twice.
///
/// `--measure` goes further and **runs** them. That is a different kind of
/// answer: the static pass leaves whole templates undecidable by construction
/// (nothing in a card ties a subject to a predicate, so `top-reach` cannot be
/// decided from one), and no amount of card-only reasoning closes that. A run
/// closes it, and records what the answer cost.
pub(crate) fn card_audit_cmd(path: &str, opts: &AuditOptions) -> anyhow::Result<()> {
    if !opts.measure && (!opts.only.is_empty() || opts.max_mb > 0.0) {
        // Silently ignoring them would report a static audit under a flag that
        // says a run was bounded.
        anyhow::bail!("--only and --max-mb bound a measurement; add --measure");
    }
    let read = read_card_for_audit(path)?;
    let (build, measured, stored_ti) = (read.build, read.text_index, read.stored_text_index);
    let Some(card) = read.card else {
        if opts.measure || opts.write_costs {
            anyhow::bail!("{path}: no dataset card, so there are no starter queries to measure");
        }
        if opts.json {
            println!(
                "{{\"path\":{},\"card\":false,\"text_index\":{},\"findings\":[]}}",
                quote(path),
                text_index_json(measured, stored_ti)
            );
        } else {
            println!("(no dataset card)");
            if let Some(ti) = measured {
                println!("  full text     {}", ti.describe());
            }
        }
        return Ok(());
    };
    let mut findings = super::queries::audit(&card);
    let run = if opts.measure || opts.write_costs {
        Some(measure_card(
            path,
            &card,
            build.as_ref(),
            &mut findings,
            opts,
        )?)
    } else {
        None
    };
    if opts.write_costs {
        let run = run.as_ref().expect("--write-costs implies a measurement");
        let bytes = write_costs(path, &card, run, opts)?;
        eprintln!(
            "wrote {} query cost(s) into {path} — {} bytes, content hash unchanged",
            run.costs.len(),
            bytes
        );
    }

    if opts.json {
        let doc = serde_json::json!({
            "path": path,
            "card": true,
            "title": card.title,
            "triples": card.triple_count,
            "quads": card.quad_count,
            "named_graphs": card.named_graph_count,
            "queries": card.queries.len(),
            "truncated": card.truncated,
            "text_index": text_index_json(measured, stored_ti),
            "measurement": run.as_ref().map(|r| serde_json::json!({
                "transport": r.transport,
                "engine": crate::commands::buildinfo::builder_version(),
                "queries_run": r.costs.len(),
                "total_bytes": r.costs.iter().map(|c| c.bytes).sum::<u64>(),
                "total_requests": r.costs.iter().map(|c| c.requests).sum::<u64>(),
                "written": opts.write_costs,
                "note": COST_NOTE,
            })),
            "findings": findings,
        });
        println!("{}", serde_json::to_string(&doc)?);
        return Ok(());
    }

    println!("{}  ({} starter queries)", path, card.queries.len());
    // The full-text verdict leads the report: it is the one line that is true of
    // the FILE rather than of the card, and it is the question `FILTER(CONTAINS)`
    // cannot answer for you — the same query returns the same rows either way.
    match measured {
        Some(ti) => println!("  full text     {}", ti.describe()),
        None => println!(
            "  full text     unknown — a card document has no sections to measure; \
             pass the .rete itself"
        ),
    }
    if let Some(stored) = stored_ti {
        println!(
            "  full text     <- the card's own bytes claim {} — the file disagrees; re-card it",
            stored.describe()
        );
    }
    if let Some(run) = &run {
        // The transport goes ABOVE the numbers, not in a footnote: a byte
        // figure without the thing that fetched the bytes is not a cost.
        println!("  measured over: {}", run.transport);
        println!("  {COST_NOTE}");
        println!(
            "  {:<12} {:<10} {:<14} {:>7}       {:>13}   {:>6}     {:>8}",
            "card says", "run says", "query", "rows", "bytes", "req", "ms"
        );
    }
    for f in &findings {
        match &f.observed {
            None => println!(
                "  {:<12} {:<14} {:<11} {}",
                f.verdict.as_str(),
                f.id,
                f.revision,
                f.why
            ),
            Some(o) => {
                let mut note = String::new();
                if let Some(r) = &o.recorded {
                    note = if r.agrees {
                        "  = build record".to_string()
                    } else {
                        format!(
                            "  != build record ({} B, {} req, {} rows)",
                            r.bytes, r.requests, r.rows
                        )
                    };
                }
                if o.contradicts(f.verdict) {
                    note.push_str("  <- the run disagrees with the card");
                }
                println!(
                    "  {:<12} {:<10} {:<14} {:>7} row(s) {:>13} B {:>6} req {:>8} ms{note}",
                    f.verdict.as_str(),
                    o.outcome,
                    f.id,
                    o.rows,
                    o.bytes,
                    o.requests,
                    o.debug_ms,
                );
                if let Some(e) = &o.error {
                    println!("  {:<12} {:<10} {:<14} {e}", "", "", "");
                }
            }
        }
    }
    if let Some(run) = &run {
        println!(
            "  == {} quer{} run, {} bytes in {} range request(s){}",
            run.costs.len(),
            if run.costs.len() == 1 { "y" } else { "ies" },
            run.costs.iter().map(|c| c.bytes).sum::<u64>(),
            run.costs.iter().map(|c| c.requests).sum::<u64>(),
            match findings
                .iter()
                .filter_map(|f| f.observed.as_ref()?.recorded.as_ref())
                .filter(|r| !r.agrees)
                .count()
            {
                0 => String::new(),
                n => format!("  — {n} disagree with the file's own build record"),
            }
        );
    }
    Ok(())
}

/// The audit's `text_index` verdict as JSON: the measured signal, `null` when
/// nothing was measured (a card *document* has no sections), plus the card's own
/// stale claim under `card_said` when the two disagree. `null` is deliberately
/// distinguishable from `{"present": false}` — "I could not look" is not "there
/// is no index".
fn text_index_json(
    measured: Option<TextIndexSignal>,
    stored: Option<TextIndexSignal>,
) -> serde_json::Value {
    let Some(measured) = measured else {
        return serde_json::Value::Null;
    };
    let mut v = serde_json::to_value(measured).expect("TextIndexSignal serializes");
    if let Some(stored) = stored {
        v.as_object_mut().expect("an object").insert(
            "card_said".into(),
            serde_json::to_value(stored).expect("TextIndexSignal serializes"),
        );
    }
    v
}

/// What `rete card-audit` was asked to do beyond reading the card.
#[derive(Debug, Clone, Default)]
pub(crate) struct AuditOptions {
    pub json: bool,
    /// Run the queries instead of only reasoning about them.
    pub measure: bool,
    /// Measure only these ids — the way to spend 3 MB instead of 8 GB.
    pub only: Vec<String>,
    /// Give up on a query once it has asked for this many MB (0 = no cap).
    pub max_mb: f64,
    /// Record the measurement in the file's build-info section.
    pub write_costs: bool,
    /// Write even though a query measured zero rows.
    pub allow_empty: bool,
}

use crate::commands::buildinfo::COST_NOTE;

/// One measurement run: what it went through, and what each query cost.
pub(crate) struct CostRun {
    pub transport: String,
    pub costs: Vec<crate::commands::buildinfo::QueryCost>,
    /// True when only a subset of the card's queries was run (`--only`).
    pub partial: bool,
    /// Ids whose run did not finish — a byte budget bit, or the query failed.
    pub failed: Vec<String>,
}

/// Run the card's starter queries against the file that carries them and hang
/// each result on its finding.
///
/// The measurement itself is [`crate::commands::buildinfo::measure_query`] —
/// the same function `rete build` uses to fill in the build record. That is
/// deliberate and load-bearing: a re-measurement that used its own loop could
/// not be compared against a recorded one, and comparing them is the entire
/// point of re-measuring a published file.
fn measure_card(
    path: &str,
    card: &DatasetCard,
    build: Option<&super::buildinfo::BuildInfo>,
    findings: &mut [crate::commands::queries::Finding],
    opts: &AuditOptions,
) -> anyhow::Result<CostRun> {
    use crate::commands::buildinfo::{measure_query, BudgetReader};
    use crate::commands::range_source::{is_url, LocalRangeReader};

    let remote = is_url(path);
    if !remote && !path.ends_with(".rete") {
        anyhow::bail!(
            "{path}: --measure needs the .rete file itself (a card document has no data to run \
             against) — pass the local path or the http(s):// URL"
        );
    }
    // A mistyped id must not quietly narrow the run: the report would then
    // describe fewer queries than the caller asked about, and say nothing.
    let unknown: Vec<&str> = opts
        .only
        .iter()
        .map(String::as_str)
        .filter(|id| !card.queries.iter().any(|q| &q.id == id))
        .collect();
    if !unknown.is_empty() {
        anyhow::bail!(
            "no starter query matches --only {} (the card ships: {})",
            unknown.join(", "),
            card.queries
                .iter()
                .map(|q| q.id.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        );
    }
    let wanted: Vec<&ExampleQuery> = card
        .queries
        .iter()
        .filter(|q| opts.only.is_empty() || opts.only.iter().any(|id| id == &q.id))
        .collect();
    if wanted.is_empty() {
        anyhow::bail!("this card ships no starter queries to measure");
    }
    let budget = if opts.max_mb > 0.0 {
        (opts.max_mb * (1u64 << 20) as f64) as u64
    } else {
        u64::MAX
    };

    // One source reader for the whole run: neither backing store caches
    // anything (no block cache is in the stack, by design — see the transport
    // string), so sharing it costs nothing and saves one HTTP HEAD per query.
    // The per-query CountingReader is what makes each run cold.
    let source: std::sync::Arc<dyn rete_core::RangeReader + Send + Sync> = if remote {
        std::sync::Arc::new(crate::http::HttpRangeReader::open(path)?)
    } else {
        std::sync::Arc::new(LocalRangeReader::open(path)?)
    };
    let transport = format!(
        "{}; cold lazy open per query; logical range reads, no block cache; reader fan-out {}",
        if remote {
            format!("HTTP range requests to {path}")
        } else {
            format!("local file {path}")
        },
        source.concurrency()
    );

    // The file's own build record, if it has one: the known answer this run
    // gets to check itself against, query by query.
    let recorded = |id: &str| -> Option<&crate::commands::buildinfo::QueryCost> {
        build?
            .query_costs
            .as_ref()?
            .queries
            .iter()
            .find(|c| c.id == id)
    };

    let mut costs = Vec::with_capacity(wanted.len());
    let mut failed = Vec::new();
    for q in &wanted {
        let src = source.clone();
        let m = measure_query(move || Ok(BudgetReader::new(src, budget)), q)?;
        if m.error.is_some() {
            failed.push(q.id.clone());
        }
        let outcome = match (&m.error, m.cost.rows, m.vacuous) {
            (Some(_), _, _) => "error",
            (None, 0, _) => "empty",
            (None, _, true) => "vacuous",
            _ => "answers",
        };
        if let Some(f) = findings.iter_mut().find(|f| f.id == q.id) {
            f.observed = Some(crate::commands::queries::Observation {
                outcome,
                rows: m.cost.rows,
                bytes: m.cost.bytes,
                requests: m.cost.requests,
                debug_ms: m.cost.debug_ms,
                error: m.error.clone(),
                recorded: recorded(&q.id).map(|r| crate::commands::queries::Recorded {
                    bytes: r.bytes,
                    requests: r.requests,
                    rows: r.rows,
                    agrees: r.bytes == m.cost.bytes
                        && r.requests == m.cost.requests
                        && r.rows == m.cost.rows,
                }),
            });
        }
        costs.push(m.cost);
    }
    Ok(CostRun {
        transport,
        costs,
        partial: wanted.len() != card.queries.len(),
        failed,
    })
}

/// Record a measurement in the file's build-info section.
///
/// Two guards, both about not making a file worse:
///
/// * a **partial** run would store a cost list a reader reads as complete;
/// * a query that measured **zero rows** does not need a cost, it needs a
///   re-card. Recording "this greeting query costs 1 GB and answers nothing"
///   into a published file, at the price of rewriting it, is work spent making
///   the wrong thing durable. `--allow-empty` is there for the templates that
///   are honestly empty (`top-dangling` on a fully-described graph).
fn write_costs(
    path: &str,
    card: &DatasetCard,
    run: &CostRun,
    opts: &AuditOptions,
) -> anyhow::Result<u64> {
    use crate::commands::buildinfo::{
        cost_context, write_build_info_streaming, BuildInfo, QueryCosts, BUILD_INFO_SCHEMA,
    };
    use crate::commands::range_source::is_url;

    if is_url(path) {
        anyhow::bail!("--write-costs needs a local file; {path} is a URL");
    }
    if run.partial {
        anyhow::bail!(
            "--write-costs refuses a partial run: --only measured {} of the card's {} starter \
             queries, and a stored cost list reads as complete",
            run.costs.len(),
            card.queries.len()
        );
    }
    if let Some(bad) = run.failed.first() {
        anyhow::bail!(
            "refusing to write: {} of the starter queries did not finish ({bad}) — a cost record \
             is only worth storing when every figure in it is a completed run",
            run.failed.len()
        );
    }
    let empty: Vec<&str> = run
        .costs
        .iter()
        .filter(|c| c.rows == 0)
        .map(|c| c.id.as_str())
        .collect();
    if !empty.is_empty() && !opts.allow_empty {
        anyhow::bail!(
            "refusing to write: {} starter quer{} returned zero rows ({}). A file whose greeting \
             queries answer nothing needs a re-card (scripts/recard), which rewrites it anyway \
             and fixes the queries too — pass --allow-empty if the emptiness is expected",
            empty.len(),
            if empty.len() == 1 { "y" } else { "ies" },
            empty.join(", ")
        );
    }

    // Carry whatever build record the file already has; only the costs change.
    // A file built before build records existed gets a record whose honest
    // content is *just* the costs — no invented timestamp, no invented builder,
    // because this tool did not build it.
    let reader = crate::commands::range_source::LocalRangeReader::open(path)?;
    let mut info = load_card_and_build_ranged(&reader)?
        .build
        .unwrap_or(BuildInfo {
            schema: BUILD_INFO_SCHEMA,
            ..Default::default()
        });
    info.query_costs = Some(QueryCosts {
        context: cost_context(&run.transport),
        queries: run.costs.clone(),
    });
    write_build_info_streaming(std::path::Path::new(path), &info.to_json_bytes())
}

/// JSON-quote a string for the one hand-built object above.
fn quote(s: &str) -> String {
    serde_json::to_string(s).unwrap_or_else(|_| "\"\"".to_string())
}

/// Read a card — and the build record that sits in the same coalesced range —
/// from a `.rete` (local or remote, CARD tier only), or a card from a card JSON
/// document. `(no dataset card)` — what the CLI prints for a cardless file — is
/// accepted and reported as no card, so a survey's saved output can be piped
/// straight back in. A card document carries no build record; that is what
/// makes `--measure` need the file.
///
/// A `.rete` also yields the measured [`TextIndexSignal`] and any drift between
/// it and what the card's bytes claimed; a card *document* yields neither —
/// there is no file to measure, so the audit reports the index as unknown
/// rather than asserting it from a document that may be years old.
struct CardForAudit {
    card: Option<DatasetCard>,
    build: Option<super::buildinfo::BuildInfo>,
    /// `None` for a card document (nothing was measured).
    text_index: Option<TextIndexSignal>,
    stored_text_index: Option<TextIndexSignal>,
}

fn read_card_for_audit(path: &str) -> anyhow::Result<CardForAudit> {
    let from_file = |read: CardRead| CardForAudit {
        card: read.card,
        build: read.build,
        text_index: Some(read.text_index),
        stored_text_index: read.stored_text_index,
    };
    if path.starts_with("http://") || path.starts_with("https://") {
        let reader = rete_core::CountingReader::new(
            crate::commands::range_source::RangedSourceReader::open(path)?,
        );
        let out = load_card_and_build_ranged(&reader).map(from_file);
        eprintln!(
            "fetched {} bytes in {} range request(s)",
            reader.bytes_read(),
            reader.requests()
        );
        return out;
    }
    if path.ends_with(".rete") {
        let reader = crate::commands::range_source::LocalRangeReader::open(path)?;
        return load_card_and_build_ranged(&reader).map(from_file);
    }
    let text = if path == "-" {
        std::io::read_to_string(std::io::stdin())?
    } else {
        std::fs::read_to_string(path)?
    };
    let none = CardForAudit {
        card: None,
        build: None,
        text_index: None,
        stored_text_index: None,
    };
    let Some(start) = text.find('{') else {
        return Ok(none);
    };
    let card = serde_json::from_str(&text[start..])
        .map_err(|e| anyhow::anyhow!("{path}: not a dataset card document: {e}"))?;
    Ok(CardForAudit {
        card: Some(card),
        ..none
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    // The bag's bounds are the format's, enforced in `rete-core`; the tests
    // below assert this crate honours exactly those numbers.
    use rete_core::card::{CARD_EXTRA_MAX_BYTES, CARD_EXTRA_MAX_KEYS, CARD_EXTRA_MAX_KEY_BYTES};

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

    /// `signals.text_index` is measured from the file's sections at READ time,
    /// so the write path must drop it: a stored copy would be a claim that
    /// survives the sections it describes (a `repyramid --text-index` rewrites
    /// them), which is the class of drift #189 had to repair by hand.
    #[test]
    fn the_text_index_signal_is_stripped_from_the_stored_card() {
        // `causenet-full-typed.rete`'s real figures.
        let measured = TextIndexSignal {
            present: true,
            bytes: 1_879_287_762,
            token_table_bytes: Some(193_295_361),
        };
        let mut card = DatasetCard {
            title: Some("Indexed".into()),
            triple_count: 3,
            ..Default::default()
        };
        assert_eq!(card.observe_text_index(measured), None);
        assert_eq!(card.signals.text_index, Some(measured));

        let bytes = card.to_json_bytes();
        let text = String::from_utf8(bytes.clone()).unwrap();
        assert!(
            !text.contains("text_index"),
            "the signal must not reach the metadata section: {text}"
        );
        // Everything else still round-trips, and the reader gets `None` =
        // "unknown" rather than a fabricated `false`.
        let back = DatasetCard::from_json_bytes(&bytes).unwrap();
        assert_eq!(back.title, card.title);
        assert_eq!(back.signals.text_index, None);

        // Stripping is not mutation: the in-memory card keeps what was measured
        // so the SAME card can be rendered and serialized in either order.
        assert_eq!(card.signals.text_index, Some(measured));
    }

    /// The signal survives the card's own JSON envelope — which is what
    /// `rete card --json`, the wasm reader and `card-audit`'s document input all
    /// speak — even though it never reaches the file.
    #[test]
    fn the_text_index_signal_round_trips_through_a_card_document() {
        let mut card = DatasetCard {
            title: Some("Indexed".into()),
            ..Default::default()
        };
        card.observe_text_index(TextIndexSignal {
            present: true,
            bytes: 80,
            token_table_bytes: Some(68),
        });
        let doc = serde_json::to_vec(&card_json(&card)).unwrap();
        let back: DatasetCard = serde_json::from_slice(&doc).unwrap();
        assert_eq!(back.signals.text_index, card.signals.text_index);

        // A measured negative serializes as `{"present": false}` — nothing else
        // is asserted alongside it, and it is NOT the same as the field's
        // absence, which means "nobody measured".
        let mut none = DatasetCard::default();
        none.observe_text_index(TextIndexSignal::default());
        let text = serde_json::to_string(&card_json(&none)).unwrap();
        assert!(text.contains(r#""text_index":{"present":false}"#), "{text}");
        let absent = serde_json::to_string(&card_json(&DatasetCard::default())).unwrap();
        assert!(!absent.contains("text_index"), "{absent}");
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

    /// The card must report the size of the FILE, not the size of the input.
    ///
    /// Regression for #128: the card was derived from the ingested statement
    /// count while the header records the count the index kept after dedup, so
    /// any dataset built from input containing duplicate statements — which is
    /// every harvest that pages with overlapping windows — published an inflated
    /// size. The two agree for duplicate-free input, which is why it went
    /// unnoticed. Here the input has duplicates in both the default graph and a
    /// named graph, so a card taken from the ingest count cannot pass.
    #[test]
    fn card_counts_are_the_deduplicated_counts() {
        use rete_core::ingest::{self, DeferredMetadata};
        use rete_core::Rete;

        let g = |s: &str, p: &str, o: &str, graph: &str| {
            (
                s.to_string(),
                p.to_string(),
                o.to_string(),
                Some(graph.to_string()),
            )
        };
        let quads = vec![
            q("<http://ex/a>", "<http://ex/p>", "<http://ex/1>"),
            q("<http://ex/b>", "<http://ex/p>", "<http://ex/2>"),
            q("<http://ex/a>", "<http://ex/p>", "<http://ex/1>"), // duplicate
            g("<http://ex/n>", "<http://ex/q>", "\"v\"", "<http://ex/g>"),
            g("<http://ex/n>", "<http://ex/q>", "\"v\"", "<http://ex/g>"), // duplicate
        ];
        // 5 statements ingested; 3 unique (2 default + 1 named).
        let (bytes, stats) =
            ingest::assemble_dataset_with_opts(quads, false, false, None, |stats, quads| {
                let card = derive_card(
                    quads,
                    stats.terms as u64,
                    stats.named_graphs as u64,
                    CardInput::default(),
                );
                DeferredMetadata::new(move |counts| card.with_final_counts(counts).to_json_bytes())
            });

        let header = rete_core::Header::from_bytes(&bytes).unwrap();
        let card = load_card(&bytes).unwrap().expect("card embedded");
        assert_eq!(header.quad_count, 3, "header records the deduped quads");
        assert_eq!(
            card.quad_count, header.quad_count,
            "card and header must not disagree about the size of the same file"
        );
        assert_eq!(card.triple_count, 2, "default-graph triples, deduped");

        // And the index agrees with both.
        let rete = Rete::open(&bytes).unwrap();
        let default_triples = rete.dump(None).len() as u64;
        let named: u64 = rete
            .graph_names()
            .iter()
            .map(|name| rete.dump(Some(name)).len() as u64)
            .sum();
        assert_eq!(default_triples + named, card.quad_count);

        // The returned stats describe the file too — they print the `wrote …`
        // line, which used to report the raw input size.
        assert_eq!(stats.statements, 3);
        assert_eq!(stats.default_triples, 2);
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

    /// New curated identity/provenance fields round-trip through a card-file
    /// JSON document and into the card, and absent ones are omitted from the
    /// stored bytes (size discipline).
    #[test]
    fn curated_identity_fields_round_trip() {
        let input: CardInput = serde_json::from_str(
            r#"{
                "title": "T",
                "version": "2026-08-04",
                "creators": [{"name":"Ada","orcid":"https://orcid.org/0000-0002-1825-0097"}],
                "publisher": {"name":"EPFL","ror":"https://ror.org/02s376052"},
                "canonical_url": "https://data.example.org/t.rete",
                "sparql_endpoint": "https://example.org/sparql",
                "source_date": "2026-07-01",
                "derived_from": ["https://example.org/dump.nt"],
                "doi": "https://doi.org/10.5281/zenodo.1",
                "cite_as": "Ada (2026). T.",
                "keywords": ["graphs", "citations"],
                "theme": ["http://publications.europa.eu/resource/authority/data-theme/TECH"]
            }"#,
        )
        .unwrap();
        let card = curated_counts_card(5, 9, input);
        let bytes = card.to_json_bytes();
        let back = DatasetCard::from_json_bytes(&bytes).unwrap();
        assert_eq!(back.version.as_deref(), Some("2026-08-04"));
        assert_eq!(back.creators[0].name, "Ada");
        assert_eq!(
            back.creators[0].orcid.as_deref(),
            Some("https://orcid.org/0000-0002-1825-0097")
        );
        assert_eq!(back.publisher.as_ref().unwrap().name, "EPFL");
        assert_eq!(
            back.canonical_url.as_deref(),
            Some("https://data.example.org/t.rete")
        );
        assert_eq!(back.derived_from, vec!["https://example.org/dump.nt"]);
        assert_eq!(
            back.doi.as_deref(),
            Some("https://doi.org/10.5281/zenodo.1")
        );
        // Round-trip preserves what was stored (normalization is
        // `load_curated`'s job, covered by its own test below).
        assert_eq!(back.keywords, vec!["graphs", "citations"]);
        assert_eq!(
            back.theme,
            vec!["http://publications.europa.eu/resource/authority/data-theme/TECH"]
        );

        // A minimal card omits every absent identity field from its bytes.
        let plain = curated_counts_card(1, 2, CardInput::default()).to_json_bytes();
        let text = String::from_utf8(plain).unwrap();
        for absent in [
            "\"creators\"",
            "\"publisher\"",
            "\"canonical_url\"",
            "\"sparql_endpoint\"",
            "\"derived_from\"",
            "\"doi\"",
            "\"cite_as\"",
            "\"source_date\"",
            "\"version\"",
            "\"top_n\"",
            "\"keywords\"",
            "\"theme\"",
            "\"extra\"",
        ] {
            assert!(
                !text.contains(absent),
                "{absent} must be omitted when unset"
            );
        }
    }

    /// The curated discovery lists (`keywords`, `theme`) are canonicalized
    /// at write time — trimmed, sorted, deduped — so two builds whose card
    /// files list the same entries in different order produce byte-identical
    /// cards; an empty entry is a loud error, never a silent drop; and a
    /// free-text `theme` is rejected (that is what `keywords` is for — the
    /// controlled-vocabulary IRI is `theme`'s whole point).
    #[test]
    fn curated_lists_are_canonicalized_and_bad_entries_rejected() {
        assert_eq!(
            normalize_string_list(
                "keywords",
                vec![" open data ".into(), "catalog".into(), "open data".into()]
            )
            .unwrap(),
            vec!["catalog", "open data"],
            "trimmed, sorted, deduplicated"
        );
        let err = normalize_string_list("keywords", vec!["ok".into(), "   ".into()])
            .unwrap_err()
            .to_string();
        assert!(err.contains("keywords"), "names the field: {err}");

        // Themes must be controlled-vocabulary IRIs.
        normalize_themes(vec![
            "http://publications.europa.eu/resource/authority/data-theme/GOVE".into(),
        ])
        .expect("an IRI theme is accepted");
        let err = normalize_themes(vec!["government".into()])
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("keywords"),
            "points free text at `keywords`: {err}"
        );

        // The canonical form is what serializes: reordered authoring yields
        // byte-identical cards (the same property the `extra` bag pins for
        // the content hash).
        let build = |json: &str| {
            let mut input: CardInput = serde_json::from_str(json).unwrap();
            input.keywords =
                normalize_string_list("keywords", std::mem::take(&mut input.keywords)).unwrap();
            curated_counts_card(3, 5, input).to_json_bytes()
        };
        assert_eq!(
            build(r#"{"keywords":["b","a"]}"#),
            build(r#"{"keywords":["a"," b "]}"#)
        );

        // And the text catalog renders them.
        let card = curated_counts_card(
            3,
            5,
            serde_json::from_str(
                r#"{"keywords":["catalog","open data"],
                    "theme":["http://publications.europa.eu/resource/authority/data-theme/GOVE"]}"#,
            )
            .unwrap(),
        );
        let text = format_card(&card, "aa");
        assert!(text.contains("keywords     : catalog, open data"));
        assert!(text.contains(
            "theme        : http://publications.europa.eu/resource/authority/data-theme/GOVE"
        ));
    }

    /// The `extra` bag round-trips through the stored bytes, is omitted when
    /// empty, and a card written by an EXTERNAL writer (the Python client
    /// serializes the caller's dict verbatim, counts spliced in) surfaces its
    /// custom fields in this reader — the pass-through the client relies on.
    #[test]
    fn extra_round_trips_and_external_writer_cards_surface_it() {
        let input: CardInput = serde_json::from_str(
            r#"{"title":"T","extra":{"atlas:layer":84,"review":{"by":"dg","ok":true}}}"#,
        )
        .unwrap();
        let card = curated_counts_card(5, 9, input);
        let back = DatasetCard::from_json_bytes(&card.to_json_bytes()).unwrap();
        assert_eq!(back.extra, card.extra);
        assert_eq!(back.extra.get("atlas:layer"), Some(&serde_json::json!(84)));

        // The exact JSON shape `clients/python` `card_bytes` writes: curated
        // fields + the count fields + format_version, `extra` included.
        let python_shaped = r#"{
            "title":"From Python","extra":{"pipeline":"nightly"},
            "triple_count":1,"quad_count":1,"named_graph_count":0,"term_count":3,
            "format_version":5
        }"#;
        let card = DatasetCard::from_json_bytes(python_shaped.as_bytes()).unwrap();
        assert_eq!(
            card.extra.get("pipeline"),
            Some(&serde_json::json!("nightly"))
        );
    }

    /// The bounds bite exactly at the boundary — one byte / key / level over
    /// is a loud error, at the limit is accepted.
    #[test]
    fn extra_limits_bite_exactly_at_the_boundary() {
        use serde_json::{json, Value};
        let bag = |v: Vec<(String, Value)>| -> BTreeMap<String, Value> { v.into_iter().collect() };

        // Byte cap: `{"pad":"…"}` serializes to 10 + n bytes.
        let pad = |n: usize| bag(vec![("pad".into(), json!("x".repeat(n)))]);
        let at = pad(CARD_EXTRA_MAX_BYTES - 10);
        assert_eq!(serde_json::to_vec(&at).unwrap().len(), CARD_EXTRA_MAX_BYTES);
        normalize_extra(at).expect("exactly at the byte cap is accepted");
        let over = pad(CARD_EXTRA_MAX_BYTES - 9);
        assert_eq!(
            serde_json::to_vec(&over).unwrap().len(),
            CARD_EXTRA_MAX_BYTES + 1
        );
        let err = normalize_extra(over).unwrap_err().to_string();
        assert!(
            err.contains(&format!("{} bytes", CARD_EXTRA_MAX_BYTES + 1)),
            "the error states the actual size: {err}"
        );

        // Key-count cap.
        let keys = |n: usize| {
            bag((0..n)
                .map(|i| (format!("k{i:03}"), json!(1)))
                .collect::<Vec<_>>())
        };
        normalize_extra(keys(CARD_EXTRA_MAX_KEYS)).expect("at the key cap");
        assert!(normalize_extra(keys(CARD_EXTRA_MAX_KEYS + 1)).is_err());

        // Key-length cap, and the empty key.
        normalize_extra(bag(vec![("k".repeat(CARD_EXTRA_MAX_KEY_BYTES), json!(1))]))
            .expect("at the key-length cap");
        assert!(normalize_extra(bag(vec![(
            "k".repeat(CARD_EXTRA_MAX_KEY_BYTES + 1),
            json!(1)
        )]))
        .is_err());
        assert!(normalize_extra(bag(vec![(String::new(), json!(1))])).is_err());

        // Depth cap: an object of objects-of-scalars (2 levels) ok, 3 rejected
        // — records belong in Parquet companions, not the card.
        normalize_extra(bag(vec![("d".into(), json!({"a": {"b": 1}}))])).expect("at the depth cap");
        assert!(normalize_extra(bag(vec![("d".into(), json!({"a": {"b": {"c": 1}}}))])).is_err());
        assert!(normalize_extra(bag(vec![("d".into(), json!([[[1]]]))])).is_err());

        // "@context" is reserved for a future author-supplied mapping.
        let err = normalize_extra(bag(vec![("@context".into(), json!({}))]))
            .unwrap_err()
            .to_string();
        assert!(err.contains("reserved"), "loud reservation: {err}");
    }

    /// Two builds of identical input — the `extra` bag authored in different
    /// (semantically equal) key order — produce byte-identical images, hence
    /// equal blake3 content hashes. The bag is curated, so it sits INSIDE the
    /// hash; this reproducibility is what makes that placement safe.
    #[test]
    fn extra_fields_keep_the_content_hash_reproducible() {
        let build = |json: &str| {
            let mut input: CardInput = serde_json::from_str(json).unwrap();
            // The same normalization `load_curated` applies on every path.
            input.extra = normalize_extra(std::mem::take(&mut input.extra)).unwrap();
            let quads = enriched_fixture();
            let card = derive_card(&quads, 12, 0, input);
            let (image, _) =
                rete_core::ingest::assemble_dataset_with_opts(quads, true, false, None, |_, _| {
                    card.to_json_bytes()
                });
            image
        };
        let a = build(r#"{"title":"T","extra":{"b":{"y":2,"x":1},"a":[1,2]}}"#);
        let b = build(r#"{"title":"T","extra":{"a":[1,2],"b":{"x":1,"y":2}}}"#);
        assert_eq!(a, b, "reordered-input builds are byte-identical");
        let ha = Header::from_bytes(&a).unwrap().content_hash;
        assert_eq!(ha, Header::from_bytes(&b).unwrap().content_hash);

        // …and the bag IS hashed: dropping it changes the content hash
        // (tamper-evident, not cosmetic).
        let plain = build(r#"{"title":"T"}"#);
        assert_ne!(ha, Header::from_bytes(&plain).unwrap().content_hash);
    }

    /// The CARD tier's budget survives the bag: header + ONE coalesced range
    /// still fetches card (custom fields included) and build info — the
    /// 2-request property `card-url` states, pinned with `extra` present.
    #[test]
    fn card_tier_stays_two_requests_with_extra_present() {
        use std::sync::Mutex;
        struct Counting {
            data: Vec<u8>,
            reads: Mutex<Vec<(u64, u64)>>,
        }
        impl rete_core::RangeReader for Counting {
            fn len(&self) -> u64 {
                self.data.len() as u64
            }
            fn read_at(&self, offset: u64, len: u64) -> std::io::Result<Vec<u8>> {
                self.reads.lock().unwrap().push((offset, len));
                let start = offset as usize;
                let end = start
                    .checked_add(len as usize)
                    .filter(|&e| e <= self.data.len())
                    .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::UnexpectedEof, "oob"))?;
                Ok(self.data[start..end].to_vec())
            }
        }

        let input: CardInput = serde_json::from_str(
            r#"{"title":"T","extra":{"atlas:layer":"84","review":{"by":"dg","status":"ok"}}}"#,
        )
        .unwrap();
        let quads = enriched_fixture();
        let card = derive_card(&quads, 12, 0, input);
        let (image, _) =
            rete_core::ingest::assemble_dataset_with_opts(quads, true, false, None, |_, _| {
                card.to_json_bytes()
            });
        // Plus an adjacent build-info section, like every real card build.
        let image =
            rete_core::attach_build_info(&image, br#"{"schema":1,"builder":"test"}"#).unwrap();

        let reader = Counting {
            data: image,
            reads: Mutex::new(Vec::new()),
        };
        let read = load_card_and_build_ranged(&reader).unwrap();
        let got = read.card.expect("card present");
        assert_eq!(got.extra.get("atlas:layer"), Some(&serde_json::json!("84")));
        assert!(
            read.build.is_some(),
            "build info came out of the same range"
        );
        // No TEXT_INDEX section: the signal is answered by the header alone.
        assert_eq!(got.signals.text_index, Some(TextIndexSignal::default()));

        let reads = reader.reads.lock().unwrap();
        assert_eq!(
            reads.len(),
            2,
            "CARD tier = header + ONE coalesced range, extra included: {reads:?}"
        );
        assert_eq!(reads[0], (0, rete_core::HEADER_LEN as u64));
    }

    /// A card whose own bytes claim a full-text index the file does not have is
    /// the exact drift #189 had to repair by hand, one level down. Our writers
    /// cannot produce it ([`DatasetCard::to_json_bytes`] strips the field), but a
    /// third-party writer can — so the reader measures, overrides, and hands the
    /// stale claim back for `card-audit` to report.
    #[test]
    fn a_card_claiming_an_index_the_file_lacks_is_reported_as_drift() {
        let quads = enriched_fixture();
        let mut card = derive_card(&quads, 12, 0, CardInput::default());
        // Serialized the way a FOREIGN writer would — `to_json_bytes` would
        // strip exactly the field this test needs to smuggle in.
        card.signals.text_index = Some(TextIndexSignal {
            present: true,
            bytes: 999_999,
            token_table_bytes: Some(1_234),
        });
        let blob = serde_json::to_vec(&card).unwrap();
        assert!(String::from_utf8_lossy(&blob).contains("text_index"));

        let (image, _) =
            rete_core::ingest::assemble_dataset_with_opts(quads, true, false, None, |_, _| {
                blob.clone()
            });
        let read = load_card_and_build_ranged(&rete_core::SliceReader::new(&image)).unwrap();

        // The file has no kind-6 section, so that is what the card now says…
        assert_eq!(
            read.card.unwrap().signals.text_index,
            Some(TextIndexSignal::default())
        );
        // …and the claim it displaced is reported rather than silently dropped.
        let stale = read.stored_text_index.expect("the drift is reported");
        assert!(stale.present);
        assert_eq!(stale.bytes, 999_999);
        // `card-audit --json` surfaces it under `card_said`, beside the measurement.
        let doc = text_index_json(Some(read.text_index), read.stored_text_index);
        assert_eq!(doc["present"], serde_json::json!(false));
        assert_eq!(doc["card_said"]["present"], serde_json::json!(true));
    }

    /// The same budget with a TEXT_INDEX present: one extra read, of **≤10
    /// bytes**, at the section's own offset — the leading length varint and
    /// nothing else. Measuring a 1.88 GB index must not cost 1.88 GB, nor even
    /// the token table it measures.
    #[test]
    fn measuring_the_text_index_costs_one_tiny_read() {
        use std::sync::Mutex;

        struct Counting {
            data: Vec<u8>,
            reads: Mutex<Vec<(u64, u64)>>,
        }
        impl rete_core::RangeReader for Counting {
            fn read_at(&self, offset: u64, len: u64) -> std::io::Result<Vec<u8>> {
                self.reads.lock().unwrap().push((offset, len));
                let start = offset as usize;
                let end = (start + len as usize).min(self.data.len());
                Ok(self.data[start..end].to_vec())
            }
            fn len(&self) -> u64 {
                self.data.len() as u64
            }
        }

        let quads = enriched_fixture();
        let card = derive_card(&quads, 12, 0, CardInput::default());
        let (image, _) =
            rete_core::ingest::assemble_dataset_with_opts(quads, true, true, None, |_, _| {
                card.to_json_bytes()
            });
        let header = Header::from_bytes(&image).unwrap();
        assert!(header.text_index_len > 0, "the fixture is indexed");

        let reader = Counting {
            data: image,
            reads: Mutex::new(Vec::new()),
        };
        let read = load_card_and_build_ranged(&reader).unwrap();
        let signal = read.text_index;
        assert!(signal.present);
        assert_eq!(signal.bytes, header.text_index_len);
        let table = signal.token_table_bytes.expect("token table measured");
        assert!(table > 0 && table < signal.bytes);

        let reads = reader.reads.lock().unwrap();
        assert_eq!(reads.len(), 3, "header + card range + the probe: {reads:?}");
        let (offset, len) = reads[2];
        assert_eq!(
            offset, header.text_index_offset,
            "the probe reads the section's FIRST bytes"
        );
        assert!(len <= 10, "a uvarint is at most 10 bytes, not {len}");
    }

    /// A stray top-level key in a card file is a LOUD error naming the key —
    /// the enforcement that keeps the top level rete's namespace, so a future
    /// official field can never capture (or be shadowed by) a publisher's.
    #[test]
    fn top_level_custom_keys_are_rejected_not_dropped() {
        let err = serde_json::from_str::<CardInput>(r#"{"title":"T","my_field":1}"#)
            .unwrap_err()
            .to_string();
        assert!(err.contains("my_field"), "names the stray key: {err}");

        // The same content is accepted once the key moves into the bag.
        let ok: CardInput =
            serde_json::from_str(r#"{"title":"T","extra":{"my_field":1}}"#).unwrap();
        assert_eq!(ok.extra.get("my_field"), Some(&serde_json::json!(1)));
    }

    /// A card file may write `description` as an array of lines, because a
    /// Markdown description in a JSON string means hand-writing `\n` escapes.
    /// Both shapes have to land as the same single string — the card stores one
    /// string either way, so `rete card --json` feeds straight back in.
    #[test]
    fn description_reads_as_a_string_or_as_lines() {
        let from_lines: CardInput = serde_json::from_value(serde_json::json!({
            "description": ["## Contents", "", "- a", "- b"]
        }))
        .unwrap();
        assert_eq!(
            from_lines.description.as_deref(),
            Some("## Contents\n\n- a\n- b")
        );
        let from_string: CardInput =
            serde_json::from_str("{\"description\":\"## Contents\\n\\n- a\\n- b\"}").unwrap();
        assert_eq!(from_string.description, from_lines.description);

        // An absent description stays absent (the field is `default`ed).
        let none: CardInput = serde_json::from_str(r#"{"title":"T"}"#).unwrap();
        assert!(none.description.is_none());
        let null: CardInput = serde_json::from_str(r#"{"description":null}"#).unwrap();
        assert!(null.description.is_none());

        // The cap is the shared one, and it is reported by serde, not silently
        // truncated on the way in.
        let over = "x".repeat(rete_core::card::CARD_DESCRIPTION_MAX_BYTES + 1);
        let err = serde_json::from_str::<CardInput>(&format!(
            "{{\"description\":{}}}",
            serde_json::to_string(&over).unwrap()
        ))
        .unwrap_err()
        .to_string();
        assert!(err.contains("over the"), "{err}");
    }

    /// `--description` overrides the card file, so the cap has to be applied to
    /// the text that actually lands in the card — after the override, not before.
    #[test]
    fn the_description_flag_is_bounded_like_the_card_file() {
        let args = CardArgs {
            description: Some("x".repeat(rete_core::card::CARD_DESCRIPTION_MAX_BYTES + 1)),
            ..Default::default()
        };
        let err = load_curated(&args).unwrap_err().to_string();
        assert!(err.contains("`description`"), "{err}");
        assert!(err.contains("over the"), "{err}");

        let ok = load_curated(&CardArgs {
            description: Some("A short one.".into()),
            ..Default::default()
        })
        .unwrap();
        assert_eq!(ok.description.as_deref(), Some("A short one."));
    }

    /// The browser builder validates a card document against
    /// `rete_core::card::CURATED_CARD_FIELDS` — a list, not this struct, because
    /// wasm has no serde derive here. A list that drifted from the struct would
    /// let one writer accept what the other refuses, which is the whole failure
    /// this shared module exists to prevent. So pin them to each other in BOTH
    /// directions: every listed field must be accepted by `CardInput`, and every
    /// field `CardInput` accepts must be listed.
    #[test]
    fn curated_field_list_matches_the_deny_unknown_fields_struct() {
        for f in rete_core::card::CURATED_CARD_FIELDS {
            // Probe membership, not types: feed the key a `null` and require that
            // whatever serde complains about is a TYPE problem, never "unknown
            // field" — that is the only verdict this test is about.
            let doc = format!(r#"{{"{f}":null}}"#);
            if let Err(e) = serde_json::from_str::<CardInput>(&doc) {
                assert!(
                    !e.to_string().contains("unknown field"),
                    "CURATED_CARD_FIELDS lists `{f}`, but CardInput rejects it as unknown: {e}",
                );
            }
        }
        // The other direction: serde names the accepted set in its error, so a
        // field the struct gained without being listed shows up here.
        let err = serde_json::from_str::<CardInput>(r#"{"nope":1}"#)
            .unwrap_err()
            .to_string();
        for name in err
            .split("expected one of ")
            .nth(1)
            .expect("serde names the expected fields")
            // serde appends " at line L column C" — not a field name.
            .split(" at line ")
            .next()
            .unwrap()
            .split(", ")
            .map(|s| s.trim().trim_matches('`'))
            .filter(|s| !s.is_empty())
        {
            assert!(
                rete_core::card::CURATED_CARD_FIELDS.contains(&name),
                "CardInput accepts `{name}` but CURATED_CARD_FIELDS does not list it — \
                 the browser builder would reject a card the CLI accepts",
            );
        }
    }

    /// The playground's in-browser builder writes a card with no access to
    /// this crate: curated fields validated by `rete_core::card`, plus the four
    /// counts its build measured. Whatever it writes has to be a card THIS
    /// reader accepts — otherwise `rete card` on a downloaded browser build
    /// fails, and the round trip the builder promises is broken. Pin the shape
    /// by deserializing exactly what `rete_wasm::build_with_card` composes.
    #[test]
    fn a_browser_written_card_deserializes_here() {
        let curated = rete_core::card::validate_curated_card(&serde_json::json!({
            "title": "Built in a browser",
            "keywords": ["b", "a"],
            "theme": ["http://publications.europa.eu/resource/authority/data-theme/GOVE"],
            "creators": [{"name": "Ada", "orcid": "https://orcid.org/0000-0002-1825-0097"}],
            "extra": {"internal_id": "DS-1"},
        }))
        .expect("valid curated document");
        let composed = rete_core::card::compose_curated_card(curated, 3, 4, 1, 9, 5);
        let text = serde_json::to_string(&composed).unwrap();

        let card: DatasetCard = serde_json::from_str(&text).expect("CLI reads a browser card");
        assert_eq!(card.title.as_deref(), Some("Built in a browser"));
        assert_eq!(card.keywords, vec!["a".to_string(), "b".to_string()]);
        assert_eq!(card.creators.len(), 1);
        assert_eq!(card.triple_count, 3);
        assert_eq!(card.quad_count, 4);
        assert_eq!(card.term_count, 9);
        assert_eq!(card.format_version, 5);
        // The derived half is ABSENT, not zeroed — a browser build never
        // measured it and must not look as though it had.
        assert!(card.predicates.is_empty() && card.queries.is_empty());
        assert!(!text.contains("\"predicates\""), "{text}");
        assert!(!text.contains("\"top_n\""), "{text}");
    }

    /// Forward-compat: an OLD reader's struct (the pre-#153 field set, no
    /// `deny_unknown_fields` — mirrored here) must still deserialize a card
    /// written by THIS build, new fields and all.
    #[test]
    fn old_reader_struct_accepts_new_card_json() {
        #[derive(serde::Deserialize)]
        struct OldCard {
            title: Option<String>,
            triple_count: u64,
            #[serde(default)]
            predicates: Vec<(String, u64)>,
            format_version: u8,
        }
        let input: CardInput = serde_json::from_str(
            r#"{"title":"New","version":"1","creators":[{"name":"A"}],
                "canonical_url":"https://x/y.rete","derived_from":["https://x/d.nt"],
                "extra":{"atlas:review":"approved"}}"#,
        )
        .unwrap();
        let quads = vec![q("<http://ex/a>", "<http://ex/p>", "<http://ex/b>")];
        let card = derive_card(&quads, 3, 0, input);
        assert_eq!(card.top_n, CARD_TOP_N as u32);
        let bytes = card.to_json_bytes();
        let old: OldCard = serde_json::from_slice(&bytes).expect("old struct parses new JSON");
        assert_eq!(old.title.as_deref(), Some("New"));
        assert_eq!(old.triple_count, 1);
        assert_eq!(old.format_version, card.format_version);
        assert!(!old.predicates.is_empty());
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
