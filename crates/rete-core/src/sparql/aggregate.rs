//! GROUP BY / aggregate evaluation (SPEC.md §8). Grouping runs directly on
//! integer slot rows: group keys are `Val`s (so key hashing never resolves a
//! term), and an aggregate resolves only the values it actually needs — the
//! dictionary decode and numeric parse are memoized by the per-query resolver,
//! so a repeated literal is decoded once, not once per row.

use std::rc::Rc;

use crate::row::{Ctx, Row, Val};

use super::{as_number, fmt_num_typed, lexical, Agg, GroupSpec};

/// Group `rows` and compute aggregates, producing one row per group (group-by
/// slots keep their values; each aggregate's result lands in its result slot).
pub(super) fn aggregate(ctx: &Ctx, rows: Vec<Row>, g: &GroupSpec) -> Vec<Row> {
    use std::collections::BTreeMap;
    let by_slots: Vec<Option<usize>> = g.by.iter().map(|v| ctx.slots.slot(v)).collect();

    // BTreeMap keeps group order deterministic (by integer id / value order).
    let mut groups: BTreeMap<Vec<Option<Val>>, Vec<Row>> = BTreeMap::new();
    for r in rows {
        let key: Vec<Option<Val>> = by_slots
            .iter()
            .map(|s| s.and_then(|i| r[i].clone()))
            .collect();
        groups.entry(key).or_default().push(r);
    }
    // A grouped query with no rows still yields one (empty/zero) group.
    if g.by.is_empty() && groups.is_empty() {
        groups.insert(Vec::new(), Vec::new());
    }

    let mut out = Vec::new();
    for (key, members) in groups {
        let mut row = ctx.slots.empty_row();
        for (slot, val) in by_slots.iter().zip(key.into_iter()) {
            if let (Some(i), Some(v)) = (slot, val) {
                row[*i] = Some(v);
            }
        }
        for (res_var, agg) in &g.aggs {
            if let Some(slot) = ctx.slots.slot(res_var) {
                if let Some(val) = compute_agg(ctx, agg, &members) {
                    // Canonicalize so the computed value joins/dedups exactly
                    // like an equal dictionary term would.
                    row[slot] = Some(ctx.resolver.canon_term(&val));
                }
            }
        }
        out.push(row);
    }
    out
}

/// A variable's values across `members` as numbers (memoized by the resolver:
/// a repeated literal parses once, not once per row).
fn agg_nums(ctx: &Ctx, members: &[Row], slot: usize) -> Vec<f64> {
    members
        .iter()
        .filter_map(|m| m[slot].as_ref())
        .filter_map(|v| ctx.resolver.num(v))
        .collect()
}

/// A variable's values across `members` as term strings (memoized decode).
fn agg_terms(ctx: &Ctx, members: &[Row], slot: usize) -> Vec<Rc<str>> {
    members
        .iter()
        .filter_map(|m| m[slot].as_ref())
        .filter_map(|v| ctx.resolver.str_of(v))
        .collect()
}

fn compute_agg(ctx: &Ctx, agg: &Agg, members: &[Row]) -> Option<String> {
    let slot_of = |var: &str| ctx.slots.slot(var);
    match agg {
        Agg::CountStar { .. } => Some(fmt_num_typed(members.len() as f64)),
        Agg::Count(var, distinct) => {
            let Some(slot) = slot_of(var) else {
                return Some(fmt_num_typed(0.0));
            };
            let n = if *distinct {
                members
                    .iter()
                    .filter_map(|m| m[slot].as_ref())
                    .collect::<std::collections::BTreeSet<_>>()
                    .len()
            } else {
                members.iter().filter(|m| m[slot].is_some()).count()
            };
            Some(fmt_num_typed(n as f64))
        }
        Agg::Sum(var) => {
            let nums = match slot_of(var) {
                Some(slot) => agg_nums(ctx, members, slot),
                None => Vec::new(), // never-bound variable sums to 0
            };
            Some(fmt_num_typed(nums.iter().sum()))
        }
        Agg::Avg(var) => {
            // AVG of an empty group is defined to be 0; a group that *has* bound
            // values but some aren't numeric is a type error (unbound).
            let slot = slot_of(var)?;
            let bound = members.iter().filter(|m| m[slot].is_some()).count();
            if bound == 0 {
                return Some(fmt_num_typed(0.0));
            }
            let nums = agg_nums(ctx, members, slot);
            if nums.len() != bound {
                return None;
            }
            Some(fmt_num_typed(nums.iter().sum::<f64>() / nums.len() as f64))
        }
        Agg::Sample(var) => {
            let slot = slot_of(var)?;
            members
                .iter()
                .find_map(|m| m[slot].as_ref())
                .and_then(|v| ctx.resolver.str_of(v))
                .map(|t| t.to_string())
        }
        Agg::GroupConcat(var, sep, distinct) => {
            let terms = match slot_of(var) {
                Some(slot) => agg_terms(ctx, members, slot),
                None => Vec::new(),
            };
            let mut parts: Vec<String> = terms.iter().map(|t| lexical(t)).collect();
            if *distinct {
                let mut seen = std::collections::HashSet::new();
                parts.retain(|p| seen.insert(p.clone()));
            }
            // The concatenation is a simple literal (no datatype or language tag).
            Some(crate::terms::make_literal(&parts.join(sep), None, None))
        }
        Agg::Min(var) | Agg::Max(var) => {
            let want_min = matches!(agg, Agg::Min(_));
            let slot = slot_of(var)?;
            agg_terms(ctx, members, slot)
                .into_iter()
                .reduce(|cur, v| {
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
                .map(|t| t.to_string())
        }
    }
}
