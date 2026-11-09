//! The card annex (`0x07`, SPEC.md §4.6): the card fields too big for the hot
//! card live in a cold section of their own, so the card every reader fetches on
//! every open stays small.
//!
//! The hot card keeps [`HOT_FIELD_MAX`] bytes per field. A field of the card
//! that is bigger (`description`, `extra`: the two authored, open-ended ones)
//! moves **whole** into the annex, a JSON object with the moved fields as its
//! keys; the hot card then has no such field and instead carries a marker,
//! `"annex": {"fields": [...], "bytes": N}`, so a reader knows there is more.
//! The annex is the file's last data section (kind [`super::kind::CARD_ANNEX`]);
//! its digest is in the root's section table, so it is inside the content hash
//! (through the root digest) and `verify` checks it.
//!
//! [`split`] and [`merge`] are the two directions, on raw card JSON.

use serde_json::{Map, Value};

/// Largest a hot-card field (`description`, `extra`) may be before it moves to
/// the annex: the 8 KiB write-time cap of the hot card.
pub const HOT_FIELD_MAX: usize = 8192;

/// The card fields that may move to the annex.
pub const ANNEX_FIELDS: [&str; 2] = ["description", "extra"];

/// The hot card's key naming the annex.
pub const MARKER: &str = "annex";

/// A card split in two.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Split {
    /// The hot card (raw JSON): the card as given when nothing moved.
    pub hot: Vec<u8>,
    /// The annex (raw JSON), when something moved.
    pub annex: Option<Vec<u8>>,
}

/// Move the over-sized fields of `card` into an annex. A card with nothing
/// over [`HOT_FIELD_MAX`] comes back byte for byte (and no annex). A marker
/// the input already carries is dropped: the annex it named is not the one this
/// split produces, and the writer states its own.
pub fn split(card: &[u8]) -> Split {
    let unchanged = || Split {
        hot: card.to_vec(),
        annex: None,
    };
    // A field over the cap makes the card at least that long.
    let has_marker = card.windows(8).any(|w| w == b"\"annex\":");
    if card.len() <= HOT_FIELD_MAX && !has_marker {
        return unchanged();
    }
    let Ok(Value::Object(mut obj)) = serde_json::from_slice::<Value>(card) else {
        return unchanged();
    };
    let had_marker = obj.remove(MARKER).is_some();
    let mut moved = Map::new();
    for f in ANNEX_FIELDS {
        let big = obj
            .get(f)
            .is_some_and(|v| serde_json::to_vec(v).map_or(0, |b| b.len()) > HOT_FIELD_MAX);
        if big {
            if let Some(v) = obj.remove(f) {
                moved.insert(f.to_string(), v);
            }
        }
    }
    if moved.is_empty() {
        if !had_marker {
            return unchanged();
        }
        return Split {
            hot: serde_json::to_vec(&Value::Object(obj)).unwrap_or_else(|_| card.to_vec()),
            annex: None,
        };
    }
    let fields: Vec<Value> = moved.keys().cloned().map(Value::String).collect();
    let annex = serde_json::to_vec(&Value::Object(moved)).unwrap_or_default();
    let mut marker = Map::new();
    marker.insert("fields".into(), Value::Array(fields));
    marker.insert("bytes".into(), Value::from(annex.len() as u64));
    obj.insert(MARKER.into(), Value::Object(marker));
    Split {
        hot: serde_json::to_vec(&Value::Object(obj)).unwrap_or_else(|_| card.to_vec()),
        annex: Some(annex),
    }
}

/// Does this hot card (raw JSON) say it has an annex?
pub fn has_marker(card: &[u8]) -> bool {
    card.windows(8).any(|w| w == b"\"annex\":")
        && serde_json::from_slice::<Value>(card)
            .ok()
            .is_some_and(|v| v.get(MARKER).is_some())
}

/// The full card: the hot card with the annex's fields put back and the marker
/// removed. A hot card without a marker comes back unchanged.
pub fn merge(hot: &[u8], annex: &[u8]) -> Result<Vec<u8>, String> {
    let mut obj = match serde_json::from_slice::<Value>(hot) {
        Ok(Value::Object(o)) => o,
        _ => return Err("the card is not a JSON object".into()),
    };
    let extra = match serde_json::from_slice::<Value>(annex) {
        Ok(Value::Object(o)) => o,
        _ => return Err("the card annex is not a JSON object".into()),
    };
    obj.remove(MARKER);
    for (k, v) in extra {
        if !ANNEX_FIELDS.contains(&k.as_str()) {
            return Err(format!("the card annex carries an unknown field {k:?}"));
        }
        obj.insert(k, v);
    }
    serde_json::to_vec(&Value::Object(obj)).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn big_extra() -> Value {
        // 114 md5 strings, the shape of the OpenCitations Index card.
        let md5s: Vec<String> = (0..114).map(|i| format!("{i:032x}")).collect();
        json!({ "sources": md5s.join(","), "n": 114, "note": "x".repeat(8000) })
    }

    #[test]
    fn small_cards_pass_through_byte_for_byte() {
        let c = br#"{"title":"t","extra":{"a":1}}"#;
        let s = split(c);
        assert_eq!(s.hot, c);
        assert!(s.annex.is_none());
    }

    #[test]
    fn a_big_extra_moves_whole_and_comes_back() {
        let card = json!({"title": "t", "extra": big_extra(), "description": "short"});
        let raw = serde_json::to_vec(&card).unwrap();
        assert!(raw.len() > HOT_FIELD_MAX);
        let s = split(&raw);
        let annex = s.annex.clone().expect("extra spills");
        let hot: Value = serde_json::from_slice(&s.hot).unwrap();
        assert!(hot.get("extra").is_none(), "the field moved whole");
        assert_eq!(hot["description"], "short");
        assert_eq!(hot[MARKER]["fields"], json!(["extra"]));
        assert_eq!(hot[MARKER]["bytes"], annex.len());
        assert!(has_marker(&s.hot));
        let back: Value = serde_json::from_slice(&merge(&s.hot, &annex).unwrap()).unwrap();
        assert_eq!(back, card);
    }

    #[test]
    fn a_stale_marker_without_an_annex_is_dropped() {
        let raw = br#"{"title":"t","annex":{"fields":["extra"],"bytes":9}}"#;
        let s = split(raw);
        assert!(s.annex.is_none());
        assert!(!has_marker(&s.hot));
    }
}
