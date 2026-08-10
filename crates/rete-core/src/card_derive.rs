//! Dataset Cards — the **derived** half: the card type itself and the profile
//! computed from a graph's own statements.
//!
//! A card is an embeddable data-catalog record stored in a `.rete` file's
//! metadata section. It carries **curated** metadata (title, license, source,
//! description, created, keywords/theme, example queries, plus a bounded bag of
//! publisher-defined custom fields under `extra` — see [`card_input`]) and an
//! **auto-derived** profile (counts, top predicates and classes, vocabularies,
//! datatypes, languages, class links, hubs, signals, and the tiered
//! starter-query library of [`card_queries`]), serialized as JSON.
//!
//! # Why this lives in `rete-core` and not in the CLI
//!
//! It used to live in `rete-cli`, which is a **binary-only crate** — no `lib.rs`,
//! so nothing can link it. A `.rete` built from Python, R, Java, JavaScript or
//! the in-browser builder could therefore write the curated half of a card and
//! *never* the derived half, no matter how the CLI was fixed (#152). The
//! derivation touches nothing but `std::collections`, `serde` and this crate, so
//! there was never a technical reason for the split: it is here now, and one
//! correction holds for every surface.
//!
//! Everything that is *not* pure derivation stays in the CLI — reading a
//! `--card-file` off disk, range-reading a card over HTTP, rendering the human
//! catalog view, measuring query costs. Those are the parts that would have made
//! this module unusable from wasm.
//!
//! [`card_input`]: crate::card_input
//! [`card_queries`]: crate::card_queries

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::{Header, RDF_TYPE};
/// The embeddable dataset card. Curated fields come from `rete build` flags or a
/// `--card-file` JSON document; the statistics are derived from the data at build
/// time. Absent optional/empty fields are omitted from the JSON.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct DatasetCard {
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
    /// [`crate::card_input::normalize_string_list`] at build time: trimmed, sorted, deduplicated
    /// — `dcat:keyword` is an unordered repeated property, so sorting loses
    /// nothing and keeps the card's bytes (hence the reproducible content
    /// hash) independent of authoring order.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub keywords: Vec<String>,
    /// Curated **themes** — IRIs into a controlled vocabulary
    /// (`dcat:theme`, e.g. the EU data-theme authority). IRIs are
    /// **required** ([`crate::card_input::normalize_themes`]): a free-text theme is a keyword by
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
    /// the bag folds into the content hash; [`crate::card_input::normalize_extra`] canonicalizes
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
    /// default graph, the same quotient as [`crate::schema_summary`]. Object
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
pub struct Creator {
    pub name: String,
    /// `https://orcid.org/0000-0000-0000-0000`
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub orcid: Option<String>,
}

/// The publishing organisation, with an optional ROR IRI.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Publisher {
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
pub enum Tier {
    Card,
    Summary,
    Index,
}

/// One vetted starter query, instantiated with the dataset's real vocabulary.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExampleQuery {
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
/// Mirrors [`crate::schema_summary`] rows (sentinels `(literal)`/`(untyped)`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClassLink {
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
pub struct Signals {
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
    /// Which **index permutations** the file stores. Like
    /// [`Signals::text_index`], measured from the file at read time and
    /// stripped before the card's bytes are written; `None` means unknown (a
    /// card document with no file behind it), never "the default".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub permutations: Option<PermutationsSignal>,
}

impl Signals {
    /// True when no affordance was detected (lets the whole block be omitted).
    pub fn is_empty(&self) -> bool {
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
pub struct TextIndexSignal {
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

/// The file's **index permutation set**: how many orders of `(s, p, o)` it
/// stores, and therefore whether its planner can run a sort-merge join.
///
/// `rete build --permutations 3` writes SPO, POS and OSP only. Those three tie
/// the longest bound prefix on all eight triple-pattern shapes, so the file
/// answers **every query with the same rows, routed to the same tiles** — the
/// difference is invisible from the results, exactly like a missing
/// full-text index (#189). What it gives up is the merge join: SOP/PSO/OPS
/// exist only to hand a join two streams already sorted on the join key, and
/// they are typically ~40% of a built file. A consumer choosing between two
/// mirrors of one dataset, or deciding whether a join-heavy workload will hold
/// up, has to be able to see which it has.
///
/// **Measured at read time, never stored**, for the reasons [`TextIndexSignal`]
/// spells out and one more that is specific to this signal: which permutations
/// a file carries is a fact about its *bytes*. A stored copy would be an
/// authored claim about the file's own layout — the one class of statement the
/// file can always check for itself, in the 1 KiB header every card read has
/// already fetched. This costs **no extra read at all**.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PermutationsSignal {
    /// How many permutations are stored: 3 or 6 today.
    pub count: u8,
    /// Their names in section order, e.g. `["SPO", "POS", "OSP"]`. Written out
    /// rather than implied by `count`, because the mask is a set and a future
    /// build may keep a different three.
    pub names: Vec<String>,
    /// Whether the file carries the merge-join orders (SOP, PSO, OPS) — i.e.
    /// whether a two-pattern join sharing a same-role variable can be answered
    /// by a linear two-pointer merge instead of a hash join.
    pub merge_join: bool,
}

impl PermutationsSignal {
    /// Read it straight off the parsed header — the mask is one byte at `[50]`,
    /// and `0` there means all six (every file written before the mask
    /// existed). No range read of any kind.
    pub fn probe(header: &Header) -> Self {
        let perms = header.perms;
        PermutationsSignal {
            count: perms.len() as u8,
            names: perms.names().into_iter().map(str::to_string).collect(),
            merge_join: perms.has_merge_orders(),
        }
    }

    /// One line for the human catalog view and the audit report.
    pub fn describe(&self) -> String {
        if self.merge_join {
            format!(
                "{} index permutations ({}) — sort-merge joins available",
                self.count,
                self.names.join("/")
            )
        } else {
            format!(
                "{} index permutations ({}) — same rows, same routing; \
                 no sort-merge join (hash/probe answers instead)",
                self.count,
                self.names.join("/")
            )
        }
    }
}

impl TextIndexSignal {
    /// Measure the signal from a file's header, through the same
    /// [`crate::RangeReader`] the card was read with. Costs nothing beyond
    /// the header already in hand when there is no index, and one ≤10-byte
    /// range read when there is — never the index itself.
    pub fn probe<R: crate::RangeReader + ?Sized>(reader: &R, header: &Header) -> Self {
        if header.text_index_len == 0 {
            return TextIndexSignal::default();
        }
        TextIndexSignal {
            present: true,
            bytes: header.text_index_len,
            token_table_bytes: crate::read_text_index_token_table_len_ranged(reader, header),
        }
    }

    /// One line for the human catalog view and the audit report.
    pub fn describe(&self) -> String {
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
pub struct Coherence {
    /// True iff the reasoner found no incoherent point.
    pub coherent: bool,
    /// Number of incoherent points found.
    pub inconsistency_count: u32,
    /// `(kind, count)` histogram of incoherent points, sorted by kind.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub by_kind: Vec<(String, u32)>,
    /// What was checked, e.g. `"default"` (the default graph).
    pub scope: String,
    /// The reasoner ruleset version (`crate::REASON_RULESET`).
    pub rules: String,
    /// True iff the inferred triples were also materialized into the file.
    pub materialized: bool,
}

impl Coherence {
    /// An unstamped card has the all-default block (omitted from the JSON).
    pub fn is_empty(&self) -> bool {
        *self == Coherence::default()
    }

    /// Build the verdict from a reasoning result over the default graph. The
    /// `by_kind` histogram is `BTreeMap`-sourced (sorted) and carries no free-text
    /// `Inconsistency::detail`, so the stamp is byte-stable across rebuilds.
    pub fn from_reasoning(r: &crate::Reasoning, materialized: bool) -> Self {
        let mut hist: BTreeMap<&str, u32> = BTreeMap::new();
        for inc in &r.inconsistencies {
            *hist.entry(inc.kind).or_default() += 1;
        }
        Coherence {
            coherent: r.inconsistencies.is_empty(),
            inconsistency_count: r.inconsistencies.len() as u32,
            by_kind: hist.into_iter().map(|(k, v)| (k.to_string(), v)).collect(),
            scope: "default".to_string(),
            rules: crate::REASON_RULESET.to_string(),
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
pub struct CardInput {
    pub title: Option<String>,
    /// A string, or an array of lines joined with `\n` — see
    /// [`crate::card_input::normalize_description`]. Markdown is allowed here (raw HTML is not); the
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
/// `--card-file`. The shared rule lives in `crate::card` so the browser
/// builder accepts exactly the same two shapes.
fn de_description<'de, D>(d: D) -> Result<Option<String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    use serde::de::Error as _;
    let v = Option::<serde_json::Value>::deserialize(d)?;
    match v {
        None | Some(serde_json::Value::Null) => Ok(None),
        Some(v) => crate::card_input::normalize_description(&v)
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

    /// Parse a curated card **document** — the JSON a publisher writes for
    /// `rete build --card-file`, or a client hands its builder — applying the
    /// same write-time rules the CLI applies.
    ///
    /// This is the client-side twin of `rete-cli`'s `load_curated`, minus the
    /// file read: same `deny_unknown_fields` top level, same
    /// [`UNKNOWN_FIELD_HINT`] pointing a stray key at the `extra` bag, same
    /// [`normalize`](Self::normalize) pass afterwards. A Python/R/JS caller
    /// that parses its card this way is held to exactly the rules the CLI and
    /// the browser builder enforce, instead of the "is it an object?" check
    /// those bindings used to do on their own.
    ///
    /// [`UNKNOWN_FIELD_HINT`]: crate::card_input::UNKNOWN_FIELD_HINT
    pub fn from_json_str(text: &str) -> Result<Self, String> {
        let parsed: CardInput = serde_json::from_str(text).map_err(|e| {
            // The top level is reserved for rete-defined fields; point a stray
            // key at the bag instead of leaving a bare serde error.
            let hint = if e.to_string().contains("unknown field") {
                crate::card_input::UNKNOWN_FIELD_HINT
            } else {
                ""
            };
            format!("{e}{hint}")
        })?;
        parsed.normalize()
    }

    /// Canonicalize and bounds-check every curated field — the write-time gate
    /// each card-writing path funnels through, applied **after** any per-field
    /// override a caller layered on top of a card file.
    ///
    /// The rules themselves live in [`crate::card_input`]; this is the
    /// serde-typed face of them, so `rete build --card-file`, the browser
    /// builder and every language binding reject the same documents with the
    /// same words.
    pub fn normalize(mut self) -> Result<Self, String> {
        if let Some(d) = &self.description {
            crate::card_input::check_description_len(d)?;
        }
        self.keywords = crate::card_input::normalize_string_list(
            "keywords",
            std::mem::take(&mut self.keywords),
        )?;
        self.theme = crate::card_input::normalize_themes(std::mem::take(&mut self.theme))?;
        let bag: serde_json::Map<String, serde_json::Value> =
            std::mem::take(&mut self.extra).into_iter().collect();
        self.extra = crate::card_input::normalize_extra(bag)?
            .into_iter()
            .collect();
        Ok(self)
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
    pub fn to_json_bytes(&self) -> Vec<u8> {
        if self.signals.text_index.is_none() && self.signals.permutations.is_none() {
            return serde_json::to_vec(self).expect("DatasetCard serializes");
        }
        let mut stored = self.clone();
        stored.signals.text_index = None;
        stored.signals.permutations = None;
        serde_json::to_vec(&stored).expect("DatasetCard serializes")
    }

    /// Attach the read-time [`TextIndexSignal`], returning whatever the card's
    /// own bytes claimed before it (normally `None` — the writers strip it).
    ///
    /// A `Some(_)` return is drift: a card asserting a full-text index that no
    /// longer has to be believed, because the file itself was just measured.
    /// `rete card-audit` reports it.
    pub fn observe_text_index(&mut self, measured: TextIndexSignal) -> Option<TextIndexSignal> {
        self.signals.text_index.replace(measured)
    }

    /// Attach the read-time [`PermutationsSignal`]. Same contract as
    /// [`observe_text_index`](Self::observe_text_index): a `Some(_)` return is
    /// a card whose bytes claimed a permutation set, which the writers strip
    /// and `CardInput` cannot author.
    pub fn observe_permutations(
        &mut self,
        measured: PermutationsSignal,
    ) -> Option<PermutationsSignal> {
        self.signals.permutations.replace(measured)
    }

    /// Parse a card from the metadata-section bytes.
    ///
    /// The error is a plain `String`, like every other message in the card
    /// modules: the CLI wraps it in `anyhow`, the wasm build hands it to
    /// JavaScript, and neither needs a dependency on the other's error type.
    pub fn from_json_bytes(b: &[u8]) -> Result<Self, String> {
        serde_json::from_slice(b).map_err(|e| format!("malformed dataset card: {e}"))
    }

    /// Stamp the build-time coherence verdict (consuming builder).
    pub fn with_coherence(mut self, r: &crate::Reasoning, materialized: bool) -> Self {
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
    /// this is how it reaches the card. See `crate::FinalCounts`.
    ///
    /// The distributions (predicate/class histograms, hub degrees) stay over the
    /// derived multiset — they are shapes, not sizes.
    pub fn with_final_counts(mut self, counts: crate::ingest::FinalCounts) -> Self {
        self.triple_count = counts.default_triples;
        self.quad_count = counts.quads;
        self
    }
}

/// Cap for every top-N list embedded in the card. The metadata section is
/// fetched on **every** overview (it is part of the index-free CARD tier), so an
/// unbounded `class_links` (O(classes × predicates × classes)) or predicate list
/// would bloat that fetch on a large schema (CIDOC-CRM/MMM). Capping keeps the
/// card small and bounded; `truncated` flags when a list was actually cut.
pub const CARD_TOP_N: usize = 100;

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
// `pub`: the query generator gates its geometry template on which of
// these two the card actually recorded, so both modules must name the same IRI.
pub const GEO_ASWKT: &str = "<http://www.opengis.net/ont/geosparql#asWKT>";
pub const GEO_HASGEOMETRY: &str = "<http://www.opengis.net/ont/geosparql#hasGeometry>";
// Datatype IRIs (unbracketed — as they appear after `^^` in a literal term).
const GEO_WKTLITERAL: &str = "http://www.opengis.net/ont/geosparql#wktLiteral";
const XSD_STRING: &str = "http://www.w3.org/2001/XMLSchema#string";
const RDF_LANGSTRING: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#langString";

/// The [`ClassLink::o_class`] sentinel for a literal object (the object of that
/// row is a literal, so it has no class and no outgoing edges). `pub`:
/// the query generator reads it to tell an entity-to-entity relation — one a
/// path query can walk — from a literal-valued one.
pub const O_LITERAL: &str = "(literal)";

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
pub trait CardTripleSource {
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
pub fn curated_counts_card(statements: u64, term_count: u64, curated: CardInput) -> DatasetCard {
    let mut card = DatasetCard {
        triple_count: statements,
        quad_count: statements,
        named_graph_count: 0,
        term_count,
        format_version: crate::format::CURRENT_FORMAT_VERSION,
        ..DatasetCard::default()
    };
    curated.fill(&mut card);
    card
}

pub fn derive_card(
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
pub fn derive_card_encoded(
    dict: &crate::Dictionary,
    triples: &[(u32, u32, u32)],
    quad_count: u64,
    term_count: u64,
    named_graph_count: u64,
    curated: CardInput,
) -> DatasetCard {
    struct Encoded<'a> {
        dict: &'a crate::Dictionary,
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
/// [`crate::schema_summary`], folded in here to avoid a second full
/// materialization); the second tallies everything else. All counts are over the
/// raw (pre-dedup) default-graph multiset, matching the existing card stats and
/// `rete progressive`.
pub fn derive_card_from(
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
        // Measured by the READER from the file's header/section directory,
        // never derived from the triples and never written — see
        // `TextIndexSignal` and `PermutationsSignal`.
        text_index: None,
        permutations: None,
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
        format_version: crate::CURRENT_FORMAT_VERSION,
        ..DatasetCard::default()
    };
    curated.fill(&mut card);
    // The tiered starter-query library, instantiated from the profile above.
    card.queries = crate::card_queries::generate(&card);
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

/// Read the dataset card embedded in a `.rete` file image, or `None` if it has
/// no metadata section. Parses only the header and the metadata range — it
/// never decodes the dictionary, index or pyramid, so a client that just built
/// a file can read back the card it wrote for the price of a slice.
///
/// The ranged (remote) companion lives in `rete-cli`, which has the HTTP reader.
pub fn load_card(bytes: &[u8]) -> Result<Option<DatasetCard>, String> {
    let header = Header::from_bytes(bytes).map_err(|e| e.to_string())?;
    if header.metadata_len == 0 {
        return Ok(None);
    }
    let start = header.metadata_offset as usize;
    let end = start
        .checked_add(header.metadata_len as usize)
        .filter(|&e| e <= bytes.len())
        .ok_or_else(|| "metadata section out of bounds".to_string())?;
    Ok(Some(DatasetCard::from_json_bytes(&bytes[start..end])?))
}

#[cfg(test)]
mod tests {
    use super::*;
    // The bag's bounds are the format's, enforced in `card_input`; the tests
    // below assert the derived half honours exactly those numbers.
    use crate::card_input::{CARD_EXTRA_MAX_BYTES, CARD_EXTRA_MAX_KEYS, CARD_EXTRA_MAX_KEY_BYTES};

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

    /// `signals.permutations` is measured from the header's permutation mask at
    /// READ time. It must never reach the metadata section: a card that
    /// *claimed* a permutation set would be an authored statement about the
    /// file's own bytes — the one class of claim the file always answers for
    /// itself, for free, in the 1 KiB header every card read already fetched.
    #[test]
    fn the_permutation_signal_is_stripped_from_the_stored_card() {
        let measured = PermutationsSignal {
            count: 3,
            names: vec!["SPO".into(), "POS".into(), "OSP".into()],
            merge_join: false,
        };
        let mut card = DatasetCard {
            title: Some("Lean".into()),
            triple_count: 3,
            ..Default::default()
        };
        assert_eq!(card.observe_permutations(measured.clone()), None);
        assert_eq!(card.signals.permutations, Some(measured.clone()));

        let bytes = card.to_json_bytes();
        let text = String::from_utf8(bytes.clone()).unwrap();
        assert!(
            !text.contains("permutations"),
            "the signal must not reach the metadata section: {text}"
        );
        let back = DatasetCard::from_json_bytes(&bytes).unwrap();
        assert_eq!(back.signals.permutations, None, "unknown, not `six`");
        assert_eq!(back.title, card.title);

        // And a curated card document cannot author it: `CardInput` is
        // `deny_unknown_fields` and has no `signals` field at all.
        let authored = r#"{"title":"x","signals":{"permutations":{"count":6}}}"#;
        assert!(
            serde_json::from_str::<CardInput>(authored).is_err(),
            "a hand-written card must not be able to assert a permutation set"
        );
    }

    /// The signal is derived from the header, so it answers for a file with no
    /// card at all — and says six for every file written before the mask
    /// existed, whose byte 50 is zero.
    #[test]
    fn the_permutation_signal_is_derived_from_the_header() {
        let quads = vec![
            q("<http://ex/a>", "<http://ex/p>", "<http://ex/b>"),
            q("<http://ex/b>", "<http://ex/p>", "<http://ex/c>"),
        ];
        let (six, _) = crate::ingest::assemble_dataset_with_perms(
            quads.clone(),
            false,
            false,
            None,
            crate::PyramidAlgo::Louvain,
            crate::PermSet::ALL,
            |_, _| Vec::new(),
        );
        let (three, _) = crate::ingest::assemble_dataset_with_perms(
            quads,
            false,
            false,
            None,
            crate::PyramidAlgo::Louvain,
            crate::PermSet::CORE,
            |_, _| Vec::new(),
        );
        // Byte 50 is the mask; `0` is the canonical spelling of "all six", so a
        // default build stays byte-identical to every file written before it.
        assert_eq!(six[50], 0);
        assert_eq!(three[50], 0b0000_0111);

        let h6 = crate::Header::from_bytes(&six).unwrap();
        let h3 = crate::Header::from_bytes(&three).unwrap();
        let s6 = PermutationsSignal::probe(&h6);
        let s3 = PermutationsSignal::probe(&h3);
        assert_eq!(s6.count, 6);
        assert!(s6.merge_join);
        assert_eq!(s3.count, 3);
        assert!(!s3.merge_join);
        assert_eq!(s3.names, vec!["SPO", "POS", "OSP"]);
        assert!(s3.describe().contains("no sort-merge join"));
        // The lean file is smaller, and it is the index that shrank.
        assert!(three.len() < six.len(), "{} vs {}", three.len(), six.len());
        assert_eq!(h3.dictionary_len, h6.dictionary_len);
        assert!(h3.root_dir_len < h6.root_dir_len);
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
    /// speak — even though it never reaches the file. (Plain `serde` here, not
    /// the CLI's `card_json` wrapper: that only adds a `schemaVersion` key, and
    /// the serialization under test is this struct's.)
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
        let doc = serde_json::to_vec(&card).unwrap();
        let back: DatasetCard = serde_json::from_slice(&doc).unwrap();
        assert_eq!(back.signals.text_index, card.signals.text_index);

        // A measured negative serializes as `{"present": false}` — nothing else
        // is asserted alongside it, and it is NOT the same as the field's
        // absence, which means "nobody measured".
        let mut none = DatasetCard::default();
        none.observe_text_index(TextIndexSignal::default());
        let text = serde_json::to_string(&none).unwrap();
        assert!(text.contains(r#""text_index":{"present":false}"#), "{text}");
        let absent = serde_json::to_string(&DatasetCard::default()).unwrap();
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
        use crate::{ingest, schema_summary, Rete};
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
        use crate::ingest::{self, DeferredMetadata};
        use crate::Rete;

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

        let header = crate::Header::from_bytes(&bytes).unwrap();
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
        use crate::card_input::normalize_extra;
        use serde_json::{json, Map, Value};
        // The gate takes serde_json's `Map` (BTree-backed, so it serializes in
        // the same key order the card's `BTreeMap` does).
        let bag = |v: Vec<(String, Value)>| -> Map<String, Value> { v.into_iter().collect() };

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
            // The same normalization `rete build --card-file` applies on every
            // path — it is literally the call `load_curated` ends with.
            let input: CardInput = serde_json::from_str::<CardInput>(json)
                .unwrap()
                .normalize()
                .unwrap();
            let quads = enriched_fixture();
            let card = derive_card(&quads, 12, 0, input);
            let (image, _) =
                crate::ingest::assemble_dataset_with_opts(quads, true, false, None, |_, _| {
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
        let over = "x".repeat(crate::card::CARD_DESCRIPTION_MAX_BYTES + 1);
        let err = serde_json::from_str::<CardInput>(&format!(
            "{{\"description\":{}}}",
            serde_json::to_string(&over).unwrap()
        ))
        .unwrap_err()
        .to_string();
        assert!(err.contains("over the"), "{err}");
    }

    /// The browser builder validates a card document against
    /// `crate::card::CURATED_CARD_FIELDS` — a list, not this struct, because
    /// wasm has no serde derive here. A list that drifted from the struct would
    /// let one writer accept what the other refuses, which is the whole failure
    /// this shared module exists to prevent. So pin them to each other in BOTH
    /// directions: every listed field must be accepted by `CardInput`, and every
    /// field `CardInput` accepts must be listed.
    #[test]
    fn curated_field_list_matches_the_deny_unknown_fields_struct() {
        for f in crate::card::CURATED_CARD_FIELDS {
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
                crate::card::CURATED_CARD_FIELDS.contains(&name),
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
        let curated = crate::card::validate_curated_card(&serde_json::json!({
            "title": "Built in a browser",
            "keywords": ["b", "a"],
            "theme": ["http://publications.europa.eu/resource/authority/data-theme/GOVE"],
            "creators": [{"name": "Ada", "orcid": "https://orcid.org/0000-0002-1825-0097"}],
            "extra": {"internal_id": "DS-1"},
        }))
        .expect("valid curated document");
        let composed = crate::card::compose_curated_card(curated, 3, 4, 1, 9, 5);
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
        let r = crate::reason(&base);
        let c1 = Coherence::from_reasoning(&r, false);
        // Deterministic + the right verdict (no free-text detail in the stamp).
        assert_eq!(c1, Coherence::from_reasoning(&r, false));
        assert!(!c1.coherent);
        assert_eq!(c1.inconsistency_count, 1);
        assert_eq!(c1.by_kind, vec![("disjoint-classes".to_string(), 1)]);
        assert_eq!(c1.rules, crate::REASON_RULESET);

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
}
