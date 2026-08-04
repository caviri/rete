//! The **curated** half of a Dataset Card, and the write-time rules that guard
//! it — shared by every writer, native or wasm.
//!
//! A card has two halves. The *derived* profile (predicates, classes,
//! vocabularies, the starter-query library) is computed from the data by
//! `rete-cli` and lives there. The *curated* half is whatever a publisher
//! hands the builder — and it is the half with rules: `theme` must be an IRI
//! into a controlled vocabulary, the `extra` bag is bounded, and the top level
//! is reserved for rete-defined fields.
//!
//! Those rules used to live only in the CLI, which meant a browser build could
//! write a card the CLI would have refused. They live here so **one**
//! implementation answers for both: `rete build --card-file` and the
//! playground's in-browser builder reject the same documents with the same
//! words. [`validate_curated_card`] is the whole-document entry point;
//! [`normalize_string_list`] / [`normalize_themes`] / [`normalize_extra`] are
//! the per-field gates the CLI calls directly after serde has typed the input.
//!
//! Errors are plain `String`s: the CLI wraps them in `anyhow`, the wasm build
//! hands them to JavaScript, and neither needs a dependency on the other's
//! error type.

use serde_json::{Map, Value};

/// Every field a card *file* may carry at its top level, in the order they are
/// declared on the CLI's `CardInput` — which is also the order serde names
/// them in its "expected one of" message, so [`validate_curated_card`] can
/// reproduce that message verbatim.
///
/// The top level is **reserved**: a key not on this list is a loud error, not
/// a silent drop. That is what makes the `extra` bag's collision guarantee
/// structural rather than a naming promise — a publisher's own field can only
/// ever live inside `extra`, so a future official field can never capture one.
/// `rete-cli` pins this list against its own `deny_unknown_fields` struct by
/// test, so the two can never drift.
pub const CURATED_CARD_FIELDS: &[&str] = &[
    "title",
    "description",
    "license",
    "source",
    "created",
    "version",
    "creators",
    "publisher",
    "canonical_url",
    "sparql_endpoint",
    "source_date",
    "derived_from",
    "doi",
    "cite_as",
    "keywords",
    "theme",
    "extra",
    "example_queries",
];

/// The pointer appended to an unknown-top-level-key error. A stray key is
/// usually a typo — or a custom field that belongs in the bag — and saying so
/// costs one line and saves a round trip to the docs.
pub const UNKNOWN_FIELD_HINT: &str =
    "\n  (the card file's top level is reserved for rete-defined fields; \
     publisher-defined fields go inside the \"extra\" object — \
     see docs/dataset-cards.md)";

// --- Bounds for the publisher-defined `extra` bag, enforced by
// [`normalize_extra`] at build time (readers never validate — a bag written
// oversized by an external tool must not make the card unreadable). The
// binding constraint is SERIALIZED BYTES: custom fields ride in the metadata
// section, which every CARD-tier reader fetches on every open. ---

/// Maximum serialized size (compact JSON bytes) of the whole `extra` object.
///
/// Sized against the published catalog: 8 KiB exceeds the smallest whole card
/// (NKOD, 6,649 B stored) — generous for *metadata* — and the worst realistic
/// case, the largest card (Hugging Face, 53,580 B) + a maxed bag + its ~1 KB
/// build info ≈ 62.8 KB, still travels in the same single coalesced range
/// (the 2-request CARD tier is about request *count*, which the bag cannot
/// change). The one cost worth knowing: on the smallest cards a maxed bag can
/// push the coalesced range past a conservative TCP initial window (~14.6 KB
/// = 10 segments), i.e. one extra round trip, never an extra request.
pub const CARD_EXTRA_MAX_BYTES: usize = 8192;
/// Maximum number of keys in `extra`. A key costs ≥ 8 serialized bytes, so 64
/// typical entries sit comfortably inside [`CARD_EXTRA_MAX_BYTES`]; needing
/// more means the bag is being used as a data store — the graph itself is the
/// place for data.
pub const CARD_EXTRA_MAX_KEYS: usize = 64;
/// Maximum bytes per `extra` key. Keys are identifiers, not values.
pub const CARD_EXTRA_MAX_KEY_BYTES: usize = 128;
/// Maximum container-nesting depth inside an `extra` value (see
/// [`json_depth`]): an object of objects-of-scalars, no deeper. Deliberate —
/// deep structures invite storing *records* in the card, and Parquet
/// companions exist for records; it also hard-bounds recursion for every
/// parser that will ever read the bag (browser wasm and iOS stacks are far
/// smaller than a build machine's).
pub const CARD_EXTRA_MAX_DEPTH: usize = 2;

/// Canonicalize a curated string list (`keywords`, `theme`) — the write-time
/// gate every card-writing command funnels through (like [`normalize_extra`]).
///
/// Whitespace is trimmed and duplicates dropped; an entry that is empty after
/// trimming **rejects the build loudly** (it is always an authoring slip — a
/// stray comma in a hand-written list — and an empty `dcat:keyword` literal
/// would be projected as agreed-upon nothing). Sorting is canonicalization,
/// not editing: both fields project to unordered repeated RDF properties, so
/// no information is lost — and it keeps the card's serialization (hence the
/// reproducible content hash) independent of the order the entries were
/// authored in.
pub fn normalize_string_list(field: &str, values: Vec<String>) -> Result<Vec<String>, String> {
    let mut out: Vec<String> = Vec::with_capacity(values.len());
    for v in values {
        let trimmed = v.trim();
        if trimmed.is_empty() {
            return Err(format!("card `{field}` entries must be non-empty"));
        }
        out.push(trimmed.to_string());
    }
    out.sort();
    out.dedup();
    Ok(out)
}

/// [`normalize_string_list`] plus the `theme` requirement: every entry must be
/// an **IRI into a controlled vocabulary**. This is what keeps `theme` from
/// becoming a second keywords field — a free-text theme carries no more
/// meaning than a keyword, and `keywords` already holds those; the agreed
/// concept scheme behind the IRI is the whole value `dcat:theme` adds.
pub fn normalize_themes(themes: Vec<String>) -> Result<Vec<String>, String> {
    let themes = normalize_string_list("theme", themes)?;
    for t in &themes {
        if !(t.starts_with("http://") || t.starts_with("https://")) {
            return Err(format!(
                "card `theme` entry {t:?} is not an IRI: a theme is an IRI into a \
                 controlled vocabulary (e.g. the EU data-theme authority, \
                 http://publications.europa.eu/resource/authority/data-theme/…); \
                 free-text subjects belong in `keywords`"
            ));
        }
    }
    Ok(themes)
}

/// Canonicalize and bounds-check the `extra` bag — the write-time gate.
///
/// On overflow the build is **rejected loudly** rather than truncated quietly:
/// `extra` is authored (unlike the derived lists, which are capped with
/// `truncated` set, because they can be re-derived), so cutting it would
/// silently ship a card that no longer says what the publisher wrote — and,
/// since the bag folds into the content hash, "what I wrote" and "what hashed"
/// would diverge invisibly. Only the author can decide what to trim.
pub fn normalize_extra(extra: Map<String, Value>) -> Result<Map<String, Value>, String> {
    if extra.is_empty() {
        return Ok(extra);
    }
    if extra.len() > CARD_EXTRA_MAX_KEYS {
        return Err(format!(
            "card `extra` has {} keys, over the {CARD_EXTRA_MAX_KEYS}-key cap",
            extra.len()
        ));
    }
    let extra: Map<String, Value> = extra
        .into_iter()
        .map(|(k, v)| (k, canonicalize_json(v)))
        .collect();
    for (k, v) in &extra {
        if k.is_empty() {
            return Err("card `extra` keys must be non-empty".to_string());
        }
        if k == "@context" {
            // Reserved TODAY so it can mean something LATER: a future release
            // may honour an author-supplied JSON-LD mapping here (turning the
            // bag's projection from opaque values into the author's own
            // vocabulary). Rejecting it now keeps that door open without ever
            // breaking a published card.
            return Err("card `extra` key \"@context\" is reserved for a future \
                 author-supplied JSON-LD mapping"
                .to_string());
        }
        if k.len() > CARD_EXTRA_MAX_KEY_BYTES {
            return Err(format!(
                "card `extra` key {k:?} is {} bytes, over the {CARD_EXTRA_MAX_KEY_BYTES}-byte cap",
                k.len()
            ));
        }
        let depth = json_depth(v);
        if depth > CARD_EXTRA_MAX_DEPTH {
            return Err(format!(
                "card `extra` field {k:?} nests {depth} container levels deep, \
                 over the {CARD_EXTRA_MAX_DEPTH}-level cap"
            ));
        }
    }
    let bytes = serde_json::to_vec(&extra)
        .map_err(|e| format!("card `extra` does not serialize: {e}"))?
        .len();
    if bytes > CARD_EXTRA_MAX_BYTES {
        return Err(format!(
            "card `extra` serializes to {bytes} bytes, over the {CARD_EXTRA_MAX_BYTES}-byte cap; \
             every reader fetches the card on every open — trim the bag, \
             or put bulk data in the graph itself"
        ));
    }
    Ok(extra)
}

/// Container-nesting depth of a JSON value: scalars are 0, an array/object is
/// 1 + its deepest child. (Recursion here is bounded by serde_json's own
/// 128-level parse limit.)
pub fn json_depth(v: &Value) -> usize {
    match v {
        Value::Array(a) => 1 + a.iter().map(json_depth).max().unwrap_or(0),
        Value::Object(o) => 1 + o.values().map(json_depth).max().unwrap_or(0),
        _ => 0,
    }
}

/// Rebuild every nested object with its keys inserted in sorted order, so the
/// bag's bytes — which fold into the reproducible content hash — never depend
/// on author key order. With today's serde_json (no `preserve_order`) maps are
/// BTree-backed and this is a no-op in effect; it is insurance that a future
/// feature unification can't quietly make map order mean insertion order.
pub fn canonicalize_json(v: Value) -> Value {
    match v {
        Value::Array(a) => Value::Array(a.into_iter().map(canonicalize_json).collect()),
        Value::Object(o) => {
            let mut sorted: Vec<(String, Value)> = o.into_iter().collect();
            sorted.sort_by(|a, b| a.0.cmp(&b.0));
            let mut out = Map::new();
            for (k, v) in sorted {
                out.insert(k, canonicalize_json(v));
            }
            Value::Object(out)
        }
        scalar => scalar,
    }
}

/// Validate and canonicalize a **whole curated card document** — the JSON a
/// publisher writes for `--card-file`, or types into the playground's builder.
///
/// This is the schema-free twin of the CLI's serde-typed path: same reserved
/// top level, same field types, same `keywords`/`theme`/`extra` rules, same
/// wording. Returns the canonicalized document (lists sorted and deduplicated,
/// bag keys sorted) ready to merge with the derived counts.
///
/// It deliberately does **not** invent validation the CLI does not do: DOIs,
/// URLs, ORCIDs, RORs and dates travel as the publisher wrote them, because
/// the CLI accepts them that way and a client that rejected more would build
/// files the CLI could not reproduce — the exact drift this module exists to
/// prevent.
pub fn validate_curated_card(doc: &Value) -> Result<Value, String> {
    let obj = doc
        .as_object()
        .ok_or_else(|| "a card document must be a JSON object".to_string())?;

    for k in obj.keys() {
        if !CURATED_CARD_FIELDS.contains(&k.as_str()) {
            let expected = CURATED_CARD_FIELDS
                .iter()
                .map(|f| format!("`{f}`"))
                .collect::<Vec<_>>()
                .join(", ");
            return Err(format!(
                "unknown field `{k}`, expected one of {expected}{UNKNOWN_FIELD_HINT}"
            ));
        }
    }

    let mut out = Map::new();
    for (k, v) in obj {
        if v.is_null() {
            continue; // an explicit null is "unset", not a value
        }
        let normalized = match k.as_str() {
            "title" | "description" | "license" | "source" | "created" | "version"
            | "canonical_url" | "sparql_endpoint" | "source_date" | "doi" | "cite_as" => {
                Value::String(expect_string(k, v)?)
            }
            "derived_from" | "example_queries" => Value::from(expect_string_array(k, v)?),
            "keywords" => Value::from(normalize_string_list(
                "keywords",
                expect_string_array(k, v)?,
            )?),
            "theme" => Value::from(normalize_themes(expect_string_array(k, v)?)?),
            "creators" => {
                let arr = v.as_array().ok_or_else(|| {
                    format!("card `{k}` must be an array of {{name, orcid}} objects")
                })?;
                let mut people = Vec::with_capacity(arr.len());
                for c in arr {
                    people.push(named_agent("creators", c, "orcid")?);
                }
                Value::Array(people)
            }
            "publisher" => named_agent("publisher", v, "ror")?,
            "extra" => {
                let bag = v
                    .as_object()
                    .ok_or_else(|| "card `extra` must be a JSON object".to_string())?;
                Value::Object(normalize_extra(bag.clone())?)
            }
            _ => unreachable!("field list and match arms are kept in step"),
        };
        // Drop what serializes away anyway, so an empty list authored by hand
        // reads the same as one never written — absence, not an empty shell.
        let empty = match &normalized {
            Value::String(s) => s.is_empty(),
            Value::Array(a) => a.is_empty(),
            Value::Object(o) => o.is_empty(),
            _ => false,
        };
        if !empty {
            out.insert(k.clone(), normalized);
        }
    }
    Ok(Value::Object(out))
}

/// Finish a validated curated document into a **complete card**, by adding the
/// four counts the build just measured and the format version the writer
/// stamps.
///
/// This is the whole card a writer without the CLI's derivation can honestly
/// produce. The counts are not optional in the card schema — a document
/// missing them is not a card any reader can deserialize — so they are added
/// here rather than left to each caller to remember. Everything else the CLI
/// derives (predicates, classes, vocabularies, signals, the starter-query
/// library) is simply **absent**, which is what a reader should see: no key,
/// rather than an empty list that reads like a measured zero.
pub fn compose_curated_card(
    curated: Value,
    triple_count: u64,
    quad_count: u64,
    named_graph_count: u64,
    term_count: u64,
    format_version: u8,
) -> Value {
    let mut card = curated;
    let obj = match card.as_object_mut() {
        Some(o) => o,
        None => return card,
    };
    obj.insert("triple_count".into(), triple_count.into());
    obj.insert("quad_count".into(), quad_count.into());
    obj.insert("named_graph_count".into(), named_graph_count.into());
    obj.insert("term_count".into(), term_count.into());
    obj.insert("format_version".into(), format_version.into());
    card
}

fn expect_string(field: &str, v: &Value) -> Result<String, String> {
    v.as_str()
        .map(|s| s.trim().to_string())
        .ok_or_else(|| format!("card `{field}` must be a string"))
}

fn expect_string_array(field: &str, v: &Value) -> Result<Vec<String>, String> {
    let arr = v
        .as_array()
        .ok_or_else(|| format!("card `{field}` must be an array of strings"))?;
    arr.iter()
        .map(|e| {
            e.as_str()
                .map(|s| s.to_string())
                .ok_or_else(|| format!("card `{field}` must be an array of strings"))
        })
        .collect()
}

/// A `{name, <id>}` object — `creators` entries (`orcid`) and `publisher`
/// (`ror`). `name` is required, matching the CLI's non-optional field; the
/// identifier is optional and unvalidated, as there.
fn named_agent(field: &str, v: &Value, id_key: &str) -> Result<Value, String> {
    let o = v
        .as_object()
        .ok_or_else(|| format!("card `{field}` entries must be objects with a `name`"))?;
    for k in o.keys() {
        if k != "name" && k != id_key {
            return Err(format!(
                "unknown field `{k}` in card `{field}`, expected one of `name`, `{id_key}`"
            ));
        }
    }
    let name = o
        .get("name")
        .and_then(|n| n.as_str())
        .ok_or_else(|| format!("card `{field}` entries need a `name`"))?;
    let mut out = Map::new();
    out.insert("name".into(), Value::String(name.trim().to_string()));
    if let Some(id) = o.get(id_key).and_then(|i| i.as_str()) {
        if !id.trim().is_empty() {
            out.insert(id_key.into(), Value::String(id.trim().to_string()));
        }
    }
    Ok(Value::Object(out))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn unknown_top_level_key_is_rejected_with_the_bag_pointer() {
        let e = validate_curated_card(&json!({"title": "t", "region": "CH"})).unwrap_err();
        assert!(e.contains("unknown field `region`"), "{e}");
        assert!(e.contains("\"extra\""), "{e}");
        // The "expected one of" list must name the real fields, so a reader can
        // fix a typo without opening the docs.
        assert!(e.contains("`canonical_url`"), "{e}");
    }

    #[test]
    fn theme_must_be_an_iri_and_points_at_keywords() {
        let e = validate_curated_card(&json!({"theme": ["physics"]})).unwrap_err();
        assert!(e.contains("not an IRI"), "{e}");
        assert!(e.contains("keywords"), "{e}");
        let ok = validate_curated_card(&json!({"theme": ["https://www.wikidata.org/entity/Q413"]}))
            .unwrap();
        assert_eq!(ok["theme"][0], "https://www.wikidata.org/entity/Q413");
    }

    #[test]
    fn keywords_and_theme_are_sorted_and_deduplicated() {
        let ok = validate_curated_card(&json!({"keywords": ["zeta", "alpha", "zeta"]})).unwrap();
        assert_eq!(ok["keywords"], json!(["alpha", "zeta"]));
    }

    #[test]
    fn extra_limits_are_enforced_and_nested_keys_sorted() {
        // Depth 2 is allowed, depth 3 is not.
        let ok = validate_curated_card(&json!({"extra": {"review": {"z": 1, "a": 2}}})).unwrap();
        assert_eq!(
            serde_json::to_string(&ok["extra"]).unwrap(),
            r#"{"review":{"a":2,"z":1}}"#
        );
        let e =
            validate_curated_card(&json!({"extra": {"deep": {"a": {"b": {"c": 1}}}}})).unwrap_err();
        assert!(e.contains("over the 2-level cap"), "{e}");

        let e = validate_curated_card(&json!({"extra": {"@context": "x"}})).unwrap_err();
        assert!(e.contains("reserved"), "{e}");

        let big = "x".repeat(CARD_EXTRA_MAX_BYTES);
        let e = validate_curated_card(&json!({"extra": {"blob": big}})).unwrap_err();
        assert!(e.contains("over the 8192-byte cap"), "{e}");
    }

    #[test]
    fn creators_and_publisher_keep_their_identifiers() {
        let ok = validate_curated_card(&json!({
            "creators": [{"name": " Ada ", "orcid": "https://orcid.org/0000-0002-1825-0097"}],
            "publisher": {"name": "CERN", "ror": "https://ror.org/01ggx4157"},
        }))
        .unwrap();
        assert_eq!(ok["creators"][0]["name"], "Ada");
        assert_eq!(
            ok["creators"][0]["orcid"],
            "https://orcid.org/0000-0002-1825-0097"
        );
        assert_eq!(ok["publisher"]["ror"], "https://ror.org/01ggx4157");

        let e = validate_curated_card(&json!({"creators": [{"orcid": "x"}]})).unwrap_err();
        assert!(e.contains("need a `name`"), "{e}");
        let e =
            validate_curated_card(&json!({"publisher": {"name": "X", "isni": "y"}})).unwrap_err();
        assert!(e.contains("unknown field `isni`"), "{e}");
    }

    #[test]
    fn empty_and_null_curated_values_are_absent_not_empty() {
        let ok = validate_curated_card(&json!({
            "title": "t", "description": "", "keywords": [], "doi": null, "extra": {}
        }))
        .unwrap();
        let o = ok.as_object().unwrap();
        assert!(o.contains_key("title"));
        for absent in ["description", "keywords", "doi", "extra"] {
            assert!(
                !o.contains_key(absent),
                "{absent} should be absent, not empty"
            );
        }
    }
}
