//! The `communities` command: recompute the Louvain partition from the file's
//! index and expose per-community membership + literal corpus (the LDA input
//! described in docs/topic-modeling.md), optionally with a no-ML topic profile.

use rete_core::{Rete, DEFAULT_TILE_BUDGET};

use crate::commands::render::literal_lexical;
use crate::commands::range_source::open_local;

/// One community's membership + literal corpus, ready to serialize.
struct CommunityRecord {
    community: usize,
    members: Vec<String>,
    text: Vec<String>,
    /// Structural "topic" profile (top-K, count desc): rdf:type classes,
    /// predicates, and literal words. Empty unless profiling was requested.
    top_types: Vec<(String, u32)>,
    top_predicates: Vec<(String, u32)>,
    top_terms: Vec<(String, u32)>,
}

/// Top-K `(key, count)` from a frequency map, by count desc then key asc.
fn top_k(counts: std::collections::HashMap<String, u32>, k: usize) -> Vec<(String, u32)> {
    let mut v: Vec<(String, u32)> = counts.into_iter().collect();
    v.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    v.truncate(k);
    v
}

/// Split a literal's lexical value into lowercased content words (≥3 chars, not
/// a common stop word) for the no-ML word profile.
fn content_words(text: &str) -> impl Iterator<Item = String> + '_ {
    const STOP: &[&str] = &[
        "the", "and", "for", "with", "that", "this", "from", "are", "was", "were", "has", "have",
        "had", "into", "over", "its", "their", "they", "them", "but", "not", "all", "can", "via",
        "use", "uses", "using", "based", "study", "studies", "between", "which", "such", "these",
        "those", "than", "then", "also", "more", "most", "may", "our", "new",
    ];
    text.split(|c: char| !c.is_alphanumeric())
        .map(|w| w.to_ascii_lowercase())
        .filter(|w| w.len() >= 3 && !STOP.contains(&w.as_str()))
}

/// Recompute the Louvain communities from the opened file's index (using the
/// existing public rete-core projection/dendrogram/tiling functions — no format
/// change) and collect, per community: the distinct subject IRIs (members) and
/// the lexical values of all literal objects (the text corpus). Communities with
/// fewer than `min_size` members are dropped.
fn collect_communities(
    rete: &Rete,
    round: Option<usize>,
    min_size: usize,
    profile: bool,
    predicate: Option<&str>,
) -> anyhow::Result<Vec<CommunityRecord>> {
    use std::collections::{HashMap, HashSet};
    let dict = rete.dictionary();
    // A `--predicate` filter restricts community detection to one relation,
    // giving a criterion-specific partition (multi-criteria splitting).
    let ids = match predicate {
        None => rete.match_ids((None, None, None)),
        Some(p) => {
            let pid = dict
                .predicate_id(p)
                .ok_or_else(|| anyhow::anyhow!("predicate not found in graph: {p}"))?;
            rete.match_ids((None, Some(pid), None))
        }
    };
    let g = rete_core::project_graph(dict, &ids);
    let dend = rete_core::build_dendrogram(&g);
    let round = round.unwrap_or_else(|| {
        rete_core::choose_round_for_budget(dict, &ids, &dend, DEFAULT_TILE_BUDGET)
    });
    let tiles = rete_core::tile_by_community(dict, &ids, &dend, round);
    const RDF_TYPE: &str = "<http://www.w3.org/1999/02/22-rdf-syntax-ns#type>";
    let rdf_type_pid = dict.predicate_id(RDF_TYPE);

    let mut out = Vec::new();
    for tile in &tiles {
        // Distinct subjects (members), stable order of first appearance.
        let mut seen = HashSet::new();
        let mut members = Vec::new();
        let mut text = Vec::new();
        // Profile tallies (only filled when `profile`).
        let mut types: HashMap<String, u32> = HashMap::new();
        let mut preds: HashMap<String, u32> = HashMap::new();
        let mut terms: HashMap<String, u32> = HashMap::new();
        for &(s, p, o) in &tile.triples {
            if let Some(term) = dict.subject_term(s) {
                if seen.insert(term.clone()) {
                    members.push(term);
                }
            }
            let obj = dict.object_term(o);
            if let Some(ref obj) = obj {
                if let Some(lex) = literal_lexical(obj) {
                    if profile {
                        for w in content_words(&lex) {
                            *terms.entry(w).or_default() += 1;
                        }
                    }
                    text.push(lex);
                }
            }
            if profile {
                if let Some(pt) = dict.predicate_term(p) {
                    *preds.entry(pt).or_default() += 1;
                }
                if Some(p) == rdf_type_pid {
                    if let Some(ot) = obj {
                        *types.entry(ot).or_default() += 1;
                    }
                }
            }
        }
        if members.len() >= min_size {
            out.push(CommunityRecord {
                community: tile.community,
                members,
                text,
                top_types: if profile { top_k(types, 5) } else { Vec::new() },
                top_predicates: if profile { top_k(preds, 5) } else { Vec::new() },
                top_terms: if profile { top_k(terms, 8) } else { Vec::new() },
            });
        }
    }
    Ok(out)
}

/// `rete communities`: expose per-community membership and literal text. Human
/// form prints one line per community plus sample members; `--json` emits the
/// LDA corpus described in `docs/topic-modeling.md`.
pub(crate) fn communities(
    file: &str,
    json: bool,
    round: Option<usize>,
    min_size: usize,
    profile: bool,
    predicate: Option<&str>,
) -> anyhow::Result<()> {
    let rete = open_local(file)?;
    let records = collect_communities(&rete, round, min_size, profile, predicate)?;

    if json {
        use serde_json::{json, Value};
        let arr: Vec<Value> = records
            .iter()
            .map(|r| {
                let mut obj = json!({
                    "community": r.community,
                    "size": r.members.len(),
                    "members": r.members,
                    "text": r.text,
                });
                if profile {
                    obj["profile"] = json!({
                        "types": r.top_types,
                        "predicates": r.top_predicates,
                        "terms": r.top_terms,
                    });
                }
                obj
            })
            .collect();
        let body = json!({
            "schemaVersion": crate::JSON_SCHEMA_VERSION,
            "communities": Value::Array(arr),
        });
        println!(
            "{}",
            serde_json::to_string_pretty(&body).unwrap_or_default()
        );
        return Ok(());
    }

    if records.is_empty() {
        println!("(no communities at this round / min-size)");
        return Ok(());
    }
    let fmt_pairs = |pairs: &[(String, u32)]| {
        pairs
            .iter()
            .map(|(k, n)| format!("{k} ({n})"))
            .collect::<Vec<_>>()
            .join(", ")
    };
    for r in &records {
        println!(
            "community {}: {} members, {} literals",
            r.community,
            r.members.len(),
            r.text.len()
        );
        if profile {
            // The structural "topic" of the community — no ML needed.
            if !r.top_terms.is_empty() {
                println!("    topic words : {}", fmt_pairs(&r.top_terms));
            }
            if !r.top_types.is_empty() {
                println!("    classes     : {}", fmt_pairs(&r.top_types));
            }
            if !r.top_predicates.is_empty() {
                println!("    predicates  : {}", fmt_pairs(&r.top_predicates));
            }
        }
        for m in r.members.iter().take(5) {
            println!("    {m}");
        }
        if r.members.len() > 5 {
            println!("    … ({} more)", r.members.len() - 5);
        }
    }
    Ok(())
}
