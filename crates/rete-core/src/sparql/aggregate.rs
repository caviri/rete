//! GROUP BY / aggregate evaluation (SPEC.md §8). Grouping runs on integer
//! bindings where possible (`aggregate_int`), resolving only the group keys and
//! the values an aggregate actually needs — the dictionary decode is memoized by
//! node id so a repeated literal is decoded once, not once per row.

use crate::bgp::{term_of_value, Binding, IntBinding};
use crate::file::Rete;

use super::{as_number, fmt_num, lexical, Agg, GroupSpec};

/// Group `rows` and compute aggregates, producing one binding per group.
pub(super) fn aggregate(rows: Vec<Binding>, g: &GroupSpec) -> Vec<Binding> {
    use std::collections::BTreeMap;
    let mut groups: BTreeMap<Vec<String>, Vec<Binding>> = BTreeMap::new();
    for r in rows {
        let key: Vec<String> =
            g.by.iter()
                .map(|v| r.get(v).cloned().unwrap_or_default())
                .collect();
        groups.entry(key).or_default().push(r);
    }
    // A grouped query with no rows still yields one (empty/zero) group.
    if g.by.is_empty() && groups.is_empty() {
        groups.insert(Vec::new(), Vec::new());
    }

    let mut out = Vec::new();
    for (key, members) in groups {
        let mut b = Binding::new();
        for (v, val) in g.by.iter().zip(key.iter()) {
            if !val.is_empty() {
                b.insert(v.clone(), val.clone());
            }
        }
        for (res_var, agg) in &g.aggs {
            if let Some(val) = compute_agg(agg, &members) {
                b.insert(res_var.clone(), val);
            }
        }
        out.push(b);
    }
    out
}

/// Integer-binding aggregation: group on tagged i64 keys and resolve only the
/// group-key terms (and any values an aggregate actually needs). Mirrors
/// [`aggregate`] but avoids resolving every row to terms.
pub(super) fn aggregate_int(rete: &Rete, rows: Vec<IntBinding>, g: &GroupSpec) -> Vec<Binding> {
    use std::collections::BTreeMap;
    let dict = rete.dictionary();
    const UNBOUND: i64 = i64::MIN;

    let mut groups: BTreeMap<Vec<i64>, Vec<IntBinding>> = BTreeMap::new();
    for r in rows {
        let key: Vec<i64> =
            g.by.iter()
                .map(|v| r.get(v).copied().unwrap_or(UNBOUND))
                .collect();
        groups.entry(key).or_default().push(r);
    }
    if g.by.is_empty() && groups.is_empty() {
        groups.insert(Vec::new(), Vec::new());
    }

    let mut out = Vec::new();
    for (key, members) in groups {
        let mut b = Binding::new();
        for (v, &val) in g.by.iter().zip(key.iter()) {
            if val != UNBOUND {
                if let Some(t) = term_of_value(dict, val) {
                    b.insert(v.clone(), t);
                }
            }
        }
        for (res_var, agg) in &g.aggs {
            if let Some(val) = compute_agg_int(rete, agg, &members) {
                b.insert(res_var.clone(), val);
            }
        }
        out.push(b);
    }
    out
}

/// A variable's values across `members`, resolved to numbers, memoizing the
/// dictionary lookup + parse by node id (members frequently repeat values — e.g.
/// an integer literal — so this is ~distinct-values decodes, not one per row).
fn agg_nums(dict: &crate::Dictionary, members: &[IntBinding], var: &str) -> Vec<f64> {
    let mut cache: std::collections::HashMap<i64, Option<f64>> = std::collections::HashMap::new();
    members
        .iter()
        .filter_map(|m| m.get(var).copied())
        .filter_map(|v| {
            *cache
                .entry(v)
                .or_insert_with(|| term_of_value(dict, v).as_deref().and_then(as_number))
        })
        .collect()
}

/// A variable's values across `members`, resolved to terms, memoizing the
/// dictionary lookup by node id.
fn agg_terms(dict: &crate::Dictionary, members: &[IntBinding], var: &str) -> Vec<String> {
    let mut cache: std::collections::HashMap<i64, Option<String>> =
        std::collections::HashMap::new();
    members
        .iter()
        .filter_map(|m| m.get(var).copied())
        .filter_map(|v| {
            cache
                .entry(v)
                .or_insert_with(|| term_of_value(dict, v))
                .clone()
        })
        .collect()
}

fn compute_agg_int(rete: &Rete, agg: &Agg, members: &[IntBinding]) -> Option<String> {
    let dict = rete.dictionary();
    match agg {
        Agg::CountStar { .. } => Some(fmt_num(members.len() as f64)),
        Agg::Count(var, distinct) => {
            let n = if *distinct {
                members
                    .iter()
                    .filter_map(|m| m.get(var).copied())
                    .collect::<std::collections::BTreeSet<_>>()
                    .len()
            } else {
                members.iter().filter(|m| m.contains_key(var)).count()
            };
            Some(fmt_num(n as f64))
        }
        Agg::Sum(var) => Some(fmt_num(agg_nums(dict, members, var).iter().sum())),
        Agg::Avg(var) => {
            let v = agg_nums(dict, members, var);
            (!v.is_empty()).then(|| fmt_num(v.iter().sum::<f64>() / v.len() as f64))
        }
        Agg::Sample(var) => members
            .iter()
            .find_map(|m| m.get(var))
            .and_then(|&v| term_of_value(dict, v)),
        Agg::GroupConcat(var, sep) => Some(
            agg_terms(dict, members, var)
                .iter()
                .map(|t| lexical(t))
                .collect::<Vec<_>>()
                .join(sep),
        ),
        Agg::Min(var) | Agg::Max(var) => {
            let want_min = matches!(agg, Agg::Min(_));
            agg_terms(dict, members, var).into_iter().reduce(|cur, v| {
                let take = match (as_number(&v), as_number(&cur)) {
                    (Some(a), Some(b)) if want_min => a < b,
                    (Some(a), Some(b)) => a > b,
                    _ if want_min => v < cur,
                    _ => v > cur,
                };
                if take {
                    v
                } else {
                    cur
                }
            })
        }
    }
}

fn compute_agg(agg: &Agg, members: &[Binding]) -> Option<String> {
    let nums = |var: &str| -> Vec<f64> {
        members
            .iter()
            .filter_map(|m| m.get(var))
            .filter_map(|v| as_number(v))
            .collect()
    };
    match agg {
        Agg::CountStar { .. } => Some(fmt_num(members.len() as f64)),
        Agg::Count(var, distinct) => {
            let n = if *distinct {
                members
                    .iter()
                    .filter_map(|m| m.get(var).cloned())
                    .collect::<std::collections::BTreeSet<_>>()
                    .len()
            } else {
                members.iter().filter(|m| m.contains_key(var)).count()
            };
            Some(fmt_num(n as f64))
        }
        Agg::Sum(var) => Some(fmt_num(nums(var).iter().sum())),
        Agg::Avg(var) => {
            let v = nums(var);
            if v.is_empty() {
                None
            } else {
                Some(fmt_num(v.iter().sum::<f64>() / v.len() as f64))
            }
        }
        Agg::Min(var) => extreme(members, var, true),
        Agg::Max(var) => extreme(members, var, false),
        Agg::Sample(var) => members.iter().find_map(|m| m.get(var).cloned()),
        Agg::GroupConcat(var, sep) => Some(
            members
                .iter()
                .filter_map(|m| m.get(var))
                .map(|t| lexical(t))
                .collect::<Vec<_>>()
                .join(sep),
        ),
    }
}

/// Min (`want_min`) or Max of a variable's values: numeric when both compare as
/// numbers, else lexical.
fn extreme(members: &[Binding], var: &str, want_min: bool) -> Option<String> {
    let mut best: Option<String> = None;
    for v in members.iter().filter_map(|m| m.get(var)) {
        best = Some(match best {
            None => v.clone(),
            Some(cur) => {
                let take = match (as_number(v), as_number(&cur)) {
                    (Some(a), Some(b)) if want_min => a < b,
                    (Some(a), Some(b)) => a > b,
                    _ if want_min => v < &cur,
                    _ => v > &cur,
                };
                if take {
                    v.clone()
                } else {
                    cur
                }
            }
        });
    }
    best
}
