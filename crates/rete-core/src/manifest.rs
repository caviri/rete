//! **Manifest** — a writable *logical graph* as an ordered log of immutable
//! `.rete` segments.
//!
//! A `.rete` file is deliberately immutable: sorted delta-encoded tiles,
//! offset-chained sections, a content hash over the whole payload — that is
//! what makes it range-cacheable forever. Mutation therefore lives *outside*
//! the file, LSM-style: a small JSON **manifest** lists an ordered log of
//! entries, each contributing an optional **adds** segment and an optional
//! **dels** segment (a *tombstone* file — a plain `.rete` whose quads are the
//! deleted quads). The visible graph is the ordered fold
//!
//! ```text
//! visible = ∅
//! for entry in log:  visible = (visible ∖ entry.dels) ∪ entry.adds
//! ```
//!
//! so deletion, update (delete + insert — RDF has no in-place update), and
//! re-add-after-delete all follow from entry order. Compaction folds the log
//! into one fresh `.rete` and resets it to a single entry. The manifest is the
//! only mutable object; segments are content-addressed by the 16-byte blake3
//! content hash every `.rete` already carries in its header.
//!
//! This module is the pure data model: parse/serialize (`serde_json`, matching
//! the Dataset Card convention) and the fold. All I/O — resolving segment
//! URLs, verifying descriptors, journals — lives in the CLI layer.

use std::collections::BTreeSet;

use serde_json::{json, Value};

use crate::ingest::RawQuad;

/// The manifest format version this build reads and writes.
pub const MANIFEST_VERSION: u64 = 1;

/// Conventional manifest file suffix (a plain `.json` also parses).
pub const MANIFEST_SUFFIX: &str = ".rete-manifest.json";

/// One immutable segment: where it is and what it must be. `url` is either
/// absolute `http(s)://` or relative to the manifest's own location. `size`
/// and `blake3_16` (hex of header bytes 8..24, the content hash) pin the exact
/// artifact — the same release contract `datasets.lock.json` uses.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SegmentRef {
    /// Where the segment lives: absolute `http(s)://`, or relative to the manifest.
    pub url: String,
    /// The exact file size in bytes.
    pub size: u64,
    /// Hex of the 16-byte blake3 content hash (header bytes 8..24).
    pub blake3_16: String,
}

/// One log entry: net additions and/or net deletions (tombstones). Within an
/// entry `adds ∩ dels = ∅` by construction (a seal nets the journal per quad),
/// so their relative order does not matter; *entries* are strictly ordered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LogEntry {
    /// Quads this entry adds to the visible graph.
    pub adds: Option<SegmentRef>,
    /// Quads this entry deletes (a tombstone segment).
    pub dels: Option<SegmentRef>,
}

/// A parsed manifest: the ordered segment log plus the generation counter
/// readers use to detect staleness (one ETag-sized poll, never the data).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Manifest {
    /// The logical graph's name (segment files are conventionally named from it).
    pub name: String,
    /// Monotonic change counter: bumped by every append/seal/compact.
    pub generation: u64,
    /// The ordered segment log; the visible graph is its fold.
    pub log: Vec<LogEntry>,
}

/// Manifest parse/validation failure — the message says what and where.
#[derive(Debug)]
pub struct ManifestError(/** what failed, and where in the document */ pub String);

impl std::fmt::Display for ManifestError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "manifest: {}", self.0)
    }
}

impl std::error::Error for ManifestError {}

fn err(msg: impl Into<String>) -> ManifestError {
    ManifestError(msg.into())
}

fn segment_from_json(v: &Value, at: &str) -> Result<SegmentRef, ManifestError> {
    let url = v["url"]
        .as_str()
        .ok_or_else(|| err(format!("{at}: missing string `url`")))?;
    let size = v["size"]
        .as_u64()
        .ok_or_else(|| err(format!("{at}: missing integer `size`")))?;
    let blake3_16 = v["blake3_16"]
        .as_str()
        .ok_or_else(|| err(format!("{at}: missing string `blake3_16`")))?;
    if blake3_16.len() != 32 || !blake3_16.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(err(format!(
            "{at}: `blake3_16` must be 32 hex chars, got {blake3_16:?}"
        )));
    }
    Ok(SegmentRef {
        url: url.to_string(),
        size,
        blake3_16: blake3_16.to_ascii_lowercase(),
    })
}

fn segment_to_json(s: &SegmentRef) -> Value {
    json!({ "url": s.url, "size": s.size, "blake3_16": s.blake3_16 })
}

impl Manifest {
    /// Parse and validate a manifest JSON document.
    pub fn parse(text: &str) -> Result<Manifest, ManifestError> {
        let v: Value = serde_json::from_str(text).map_err(|e| err(format!("invalid JSON: {e}")))?;
        let version = v["rete_manifest"]
            .as_u64()
            .ok_or_else(|| err("missing integer `rete_manifest` version field"))?;
        if version != MANIFEST_VERSION {
            return Err(err(format!(
                "version {version} not supported (this build reads {MANIFEST_VERSION})"
            )));
        }
        let name = v["name"]
            .as_str()
            .ok_or_else(|| err("missing string `name`"))?
            .to_string();
        let generation = v["generation"]
            .as_u64()
            .ok_or_else(|| err("missing integer `generation`"))?;
        let log_json = v["log"]
            .as_array()
            .ok_or_else(|| err("missing array `log`"))?;
        let mut log = Vec::with_capacity(log_json.len());
        for (i, entry) in log_json.iter().enumerate() {
            let adds = match &entry["adds"] {
                Value::Null => None,
                other => Some(segment_from_json(other, &format!("log[{i}].adds"))?),
            };
            let dels = match &entry["dels"] {
                Value::Null => None,
                other => Some(segment_from_json(other, &format!("log[{i}].dels"))?),
            };
            if adds.is_none() && dels.is_none() {
                return Err(err(format!("log[{i}]: needs `adds` and/or `dels`")));
            }
            log.push(LogEntry { adds, dels });
        }
        Ok(Manifest {
            name,
            generation,
            log,
        })
    }

    /// Serialize back to the canonical pretty JSON document.
    pub fn to_json_pretty(&self) -> String {
        let log: Vec<Value> = self
            .log
            .iter()
            .map(|e| {
                let mut entry = serde_json::Map::new();
                if let Some(a) = &e.adds {
                    entry.insert("adds".into(), segment_to_json(a));
                }
                if let Some(d) = &e.dels {
                    entry.insert("dels".into(), segment_to_json(d));
                }
                Value::Object(entry)
            })
            .collect();
        let doc = json!({
            "rete_manifest": MANIFEST_VERSION,
            "name": self.name,
            "generation": self.generation,
            "log": log,
        });
        serde_json::to_string_pretty(&doc).expect("static shape") + "\n"
    }

    /// Every segment reference in log order (adds before dels within an entry),
    /// for verification/listing.
    pub fn segments(&self) -> impl Iterator<Item = (&SegmentRef, /* is_dels: */ bool)> {
        self.log.iter().flat_map(|e| {
            e.adds
                .iter()
                .map(|s| (s, false))
                .chain(e.dels.iter().map(|s| (s, true)))
        })
    }
}

/// Apply one log entry to the visible set: `(visible ∖ dels) ∪ adds`. Entries
/// must be applied in log order — that order is what makes delete-then-re-add
/// (and the reverse) mean what the writer meant.
pub fn fold_entry(
    visible: &mut BTreeSet<RawQuad>,
    dels: impl IntoIterator<Item = RawQuad>,
    adds: impl IntoIterator<Item = RawQuad>,
) {
    for q in dels {
        visible.remove(&q);
    }
    for q in adds {
        visible.insert(q);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn q(s: &str) -> RawQuad {
        (
            format!("<http://ex/{s}>"),
            "<http://ex/p>".into(),
            "<http://ex/o>".into(),
            None,
        )
    }

    fn seg(url: &str) -> SegmentRef {
        SegmentRef {
            url: url.into(),
            size: 123,
            blake3_16: "00112233445566778899aabbccddeeff".into(),
        }
    }

    #[test]
    fn roundtrip() {
        let m = Manifest {
            name: "demo".into(),
            generation: 7,
            log: vec![
                LogEntry {
                    adds: Some(seg("base.rete")),
                    dels: None,
                },
                LogEntry {
                    adds: Some(seg("delta-g2.rete")),
                    dels: Some(seg("delta-g2.tomb.rete")),
                },
            ],
        };
        let parsed = Manifest::parse(&m.to_json_pretty()).unwrap();
        assert_eq!(parsed, m);
    }

    #[test]
    fn rejects_bad_documents() {
        assert!(Manifest::parse("{}").is_err());
        assert!(Manifest::parse(
            r#"{"rete_manifest": 99, "name": "x", "generation": 0, "log": []}"#
        )
        .is_err());
        // An entry with neither adds nor dels is meaningless.
        let empty_entry = r#"{"rete_manifest": 1, "name": "x", "generation": 0, "log": [{}]}"#;
        assert!(Manifest::parse(empty_entry).is_err());
        // Hash must be 32 hex chars.
        let bad_hash = r#"{"rete_manifest": 1, "name": "x", "generation": 0,
            "log": [{"adds": {"url": "a.rete", "size": 1, "blake3_16": "zz"}}]}"#;
        assert!(Manifest::parse(bad_hash).is_err());
    }

    #[test]
    fn fold_is_ordered_last_writer_wins() {
        let mut visible = BTreeSet::new();
        // gen 1: add a, b
        fold_entry(&mut visible, [], [q("a"), q("b")]);
        // gen 2: delete a
        fold_entry(&mut visible, [q("a")], []);
        assert!(!visible.contains(&q("a")) && visible.contains(&q("b")));
        // gen 3: re-add a — later entry wins over the older tombstone
        fold_entry(&mut visible, [], [q("a")]);
        assert!(visible.contains(&q("a")));
        // deleting a quad that was never present is a no-op
        fold_entry(&mut visible, [q("ghost")], []);
        assert_eq!(visible.len(), 2);
    }

    #[test]
    fn update_is_delete_plus_insert() {
        let old = (
            "<http://ex/a>".to_string(),
            "<http://ex/name>".to_string(),
            "\"Old\"".to_string(),
            None,
        );
        let new = (
            "<http://ex/a>".to_string(),
            "<http://ex/name>".to_string(),
            "\"New\"".to_string(),
            None,
        );
        let mut visible = BTreeSet::from([old.clone()]);
        fold_entry(&mut visible, [old], [new.clone()]);
        assert_eq!(visible, BTreeSet::from([new]));
    }
}
