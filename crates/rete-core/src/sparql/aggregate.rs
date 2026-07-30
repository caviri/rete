//! GROUP BY / aggregate evaluation (SPEC.md §8). Grouping runs directly on
//! integer slot rows: group keys are `Val`s (so key hashing never resolves a
//! term), and an aggregate resolves only the values it actually needs — the
//! dictionary decode and numeric parse are memoized by the per-query resolver,
//! so a repeated literal is decoded once, not once per row.
//!
//! **Streaming.** Rows are folded through per-group [`Accum`]s one at a time, so
//! resident memory is O(number of groups × aggregates), NOT O(number of rows).
//! A `COUNT(*)` with no `GROUP BY` is a single counter — O(1) — regardless of how
//! many solutions the pattern yields. Only inherently-retaining aggregates
//! (`COUNT(DISTINCT)`, `GROUP_CONCAT`) hold per-group state, and only proportional
//! to their distinct/concatenated values.

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::rc::Rc;

use crate::row::{Ctx, Row, Val};

use super::{as_number, fmt_num_typed, lexical, Agg, GroupSpec};

/// A single aggregate's running state — updated per row, finalized once per group.
/// Slot is resolved once at construction; `None` = the variable isn't in scope.
enum Accum {
    /// COUNT(*) — every row counts (distinct-* is treated as plain count, as before).
    CountStar(u64),
    Count {
        slot: Option<usize>,
        distinct: bool,
        n: u64,
        seen: BTreeSet<Val>,
    },
    Sum {
        slot: Option<usize>,
        sum: f64,
    },
    Avg {
        slot: Option<usize>,
        bound: u64,
        sum: f64,
        num: u64,
    },
    Sample {
        slot: Option<usize>,
        done: bool,
        val: Option<Rc<str>>,
    },
    GroupConcat {
        slot: Option<usize>,
        sep: String,
        distinct: bool,
        parts: Vec<String>,
        seen: HashSet<String>,
    },
    MinMax {
        slot: Option<usize>,
        want_min: bool,
        best: Option<Rc<str>>,
    },
}

impl Accum {
    fn new(agg: &Agg, ctx: &Ctx) -> Accum {
        let slot = |v: &str| ctx.slots.slot(v);
        match agg {
            Agg::CountStar { .. } => Accum::CountStar(0),
            Agg::Count(v, d) => Accum::Count {
                slot: slot(v),
                distinct: *d,
                n: 0,
                seen: BTreeSet::new(),
            },
            Agg::Sum(v) => Accum::Sum {
                slot: slot(v),
                sum: 0.0,
            },
            Agg::Avg(v) => Accum::Avg {
                slot: slot(v),
                bound: 0,
                sum: 0.0,
                num: 0,
            },
            Agg::Sample(v) => Accum::Sample {
                slot: slot(v),
                done: false,
                val: None,
            },
            Agg::GroupConcat(v, sep, d) => Accum::GroupConcat {
                slot: slot(v),
                sep: sep.clone(),
                distinct: *d,
                parts: Vec::new(),
                seen: HashSet::new(),
            },
            Agg::Min(v) => Accum::MinMax {
                slot: slot(v),
                want_min: true,
                best: None,
            },
            Agg::Max(v) => Accum::MinMax {
                slot: slot(v),
                want_min: false,
                best: None,
            },
        }
    }

    fn update(&mut self, ctx: &Ctx, row: &Row) {
        match self {
            Accum::CountStar(n) => *n += 1,
            Accum::Count {
                slot,
                distinct,
                n,
                seen,
            } => {
                if let Some(v) = slot.and_then(|s| row[s].as_ref()) {
                    if *distinct {
                        seen.insert(v.clone());
                    } else {
                        *n += 1;
                    }
                }
            }
            Accum::Sum { slot, sum } => {
                if let Some(x) = slot
                    .and_then(|s| row[s].as_ref())
                    .and_then(|v| ctx.resolver.num(v))
                {
                    *sum += x;
                }
            }
            Accum::Avg {
                slot,
                bound,
                sum,
                num,
            } => {
                if let Some(v) = slot.and_then(|s| row[s].as_ref()) {
                    *bound += 1;
                    if let Some(x) = ctx.resolver.num(v) {
                        *sum += x;
                        *num += 1;
                    }
                }
            }
            Accum::Sample { slot, done, val } => {
                if !*done {
                    if let Some(v) = slot.and_then(|s| row[s].as_ref()) {
                        *done = true;
                        *val = ctx.resolver.str_of(v);
                    }
                }
            }
            Accum::GroupConcat {
                slot,
                distinct,
                parts,
                seen,
                ..
            } => {
                if let Some(t) = slot
                    .and_then(|s| row[s].as_ref())
                    .and_then(|v| ctx.resolver.str_of(v))
                {
                    let p = lexical(&t);
                    if !*distinct || seen.insert(p.clone()) {
                        parts.push(p);
                    }
                }
            }
            Accum::MinMax {
                slot,
                want_min,
                best,
            } => {
                if let Some(t) = slot
                    .and_then(|s| row[s].as_ref())
                    .and_then(|v| ctx.resolver.str_of(v))
                {
                    let take = match best.as_ref() {
                        None => true,
                        Some(cur) => match (as_number(&t), as_number(cur)) {
                            (Some(a), Some(b)) if *want_min => a < b,
                            (Some(a), Some(b)) => a > b,
                            _ if *want_min => t < *cur,
                            _ => t > *cur,
                        },
                    };
                    if take {
                        *best = Some(t);
                    }
                }
            }
        }
    }

    /// The group's final value in the aggregate's result slot (`None` = unbound,
    /// e.g. a type error or an out-of-scope variable).
    fn finalize(self) -> Option<String> {
        match self {
            Accum::CountStar(n) => Some(fmt_num_typed(n as f64)),
            Accum::Count {
                slot,
                distinct,
                n,
                seen,
            } => {
                if slot.is_none() {
                    return Some(fmt_num_typed(0.0)); // COUNT of an out-of-scope var is 0
                }
                let c = if distinct {
                    seen.len() as f64
                } else {
                    n as f64
                };
                Some(fmt_num_typed(c))
            }
            Accum::Sum { sum, .. } => Some(fmt_num_typed(sum)),
            Accum::Avg {
                slot,
                bound,
                sum,
                num,
            } => {
                slot?; // AVG of an out-of-scope var is unbound
                if bound == 0 {
                    return Some(fmt_num_typed(0.0)); // AVG of an empty group is 0
                }
                if num != bound {
                    return None; // a bound value wasn't numeric — type error
                }
                Some(fmt_num_typed(sum / num as f64))
            }
            Accum::Sample { val, .. } => val.map(|t| t.to_string()),
            Accum::GroupConcat { sep, parts, .. } => {
                // A simple literal (no datatype or language tag); empty group → "".
                Some(crate::terms::make_literal(&parts.join(&sep), None, None))
            }
            Accum::MinMax { best, .. } => best.map(|t| t.to_string()),
        }
    }
}

/// Group `rows` and compute aggregates, producing one row per group (group-by
/// slots keep their values; each aggregate's result lands in its result slot).
/// Rows are consumed as an iterator and folded per group — never all held at once.
pub(super) fn aggregate<I>(ctx: &Ctx, rows: I, g: &GroupSpec) -> Vec<Row>
where
    I: IntoIterator<Item = Row>,
{
    let by_slots: Vec<Option<usize>> = g.by.iter().map(|v| ctx.slots.slot(v)).collect();
    let new_accs = || -> Vec<Accum> { g.aggs.iter().map(|(_, a)| Accum::new(a, ctx)).collect() };

    // BTreeMap keeps group order deterministic (by integer id / value order).
    let mut groups: BTreeMap<Vec<Option<Val>>, Vec<Accum>> = BTreeMap::new();
    for mut r in rows {
        // Aggregate-over-expression: materialize each synthetic column on THIS row
        // before grouping, so an aggregate reads it exactly like a plain slot. An
        // expression that errors leaves the column unbound (SPARQL error semantics).
        for (var, expr) in &g.pre {
            if let Some(slot) = ctx.slots.slot(var) {
                r[slot] = expr.value(ctx, &r).map(Val::Str);
            }
        }
        let key: Vec<Option<Val>> = by_slots
            .iter()
            .map(|s| s.and_then(|i| r[i].clone()))
            .collect();
        let accs = groups.entry(key).or_insert_with(new_accs);
        for acc in accs.iter_mut() {
            acc.update(ctx, &r);
        }
    }
    // A grouped query with no rows still yields one (empty/zero) group.
    if g.by.is_empty() && groups.is_empty() {
        groups.insert(Vec::new(), new_accs());
    }

    let mut out = Vec::new();
    for (key, accs) in groups {
        let mut row = ctx.slots.empty_row();
        for (slot, val) in by_slots.iter().zip(key.into_iter()) {
            if let (Some(i), Some(v)) = (slot, val) {
                row[*i] = Some(v);
            }
        }
        for ((res_var, _), acc) in g.aggs.iter().zip(accs.into_iter()) {
            if let Some(slot) = ctx.slots.slot(res_var) {
                if let Some(val) = acc.finalize() {
                    // Canonicalize so the computed value joins/dedups exactly like
                    // an equal dictionary term would.
                    row[slot] = Some(ctx.resolver.canon_term(&val));
                }
            }
        }
        out.push(row);
    }
    out
}

#[cfg(test)]
/// Test helper: fold `members` through a single [`Accum`] — the batch view of the
/// streaming path, so the assertions below exercise the exact per-row logic.
fn compute_agg(ctx: &Ctx, agg: &Agg, members: &[Row]) -> Option<String> {
    let mut acc = Accum::new(agg, ctx);
    for m in members {
        acc.update(ctx, m);
    }
    acc.finalize()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dictionary::DictionaryBuilder;
    use crate::file::{write_file, Rete};
    use crate::index::GraphIndexBuilder;
    use crate::row::{Slots, Val};
    use crate::sparql::{ArithOp, FExpr};

    fn fixture() -> Rete {
        let triples = [("<s>", "<p>", "\"dictionary value\"")];
        let mut builder = DictionaryBuilder::new();
        for (s, p, o) in triples {
            builder.observe(s, p, o);
        }
        let dict = builder.build();
        let mut index = GraphIndexBuilder::new();
        for (s, p, o) in triples {
            index.push(dict.encode(s, p, o).unwrap());
        }
        Rete::open(&write_file(&dict, &index.build(), false, &[], 0)).unwrap()
    }

    fn val(s: &str) -> Option<Val> {
        Some(Val::Str(Rc::from(s)))
    }

    fn context(rete: &Rete) -> Ctx<'_> {
        let mut slots = Slots::new();
        for name in ["g", "v", "text", "pre", "result"] {
            slots.add(name);
        }
        Ctx::new(rete, slots)
    }

    fn row(ctx: &Ctx, group: &str, number: Option<&str>, text: Option<&str>) -> Row {
        let mut row = ctx.slots.empty_row();
        row[ctx.slots.slot("g").unwrap()] = val(group);
        row[ctx.slots.slot("v").unwrap()] = number.and_then(val);
        row[ctx.slots.slot("text").unwrap()] = text.and_then(val);
        row
    }

    #[test]
    fn aggregate_functions_cover_empty_distinct_numeric_and_error_semantics() {
        let rete = fixture();
        let ctx = context(&rete);
        let rows = vec![
            row(&ctx, "\"a\"", Some("1"), Some("\"z\"")),
            row(&ctx, "\"a\"", Some("2"), Some("\"a\"")),
            row(&ctx, "\"a\"", Some("2"), Some("\"a\"")),
            row(&ctx, "\"a\"", None, None),
        ];

        assert_eq!(
            compute_agg(&ctx, &Agg::CountStar { distinct: false }, &rows),
            Some(fmt_num_typed(4.0))
        );
        assert_eq!(
            compute_agg(&ctx, &Agg::Count("v".into(), false), &rows),
            Some(fmt_num_typed(3.0))
        );
        assert_eq!(
            compute_agg(&ctx, &Agg::Count("v".into(), true), &rows),
            Some(fmt_num_typed(2.0))
        );
        assert_eq!(
            compute_agg(&ctx, &Agg::Count("missing".into(), true), &rows),
            Some(fmt_num_typed(0.0))
        );
        assert_eq!(
            compute_agg(&ctx, &Agg::Sum("v".into()), &rows),
            Some(fmt_num_typed(5.0))
        );
        assert_eq!(
            compute_agg(&ctx, &Agg::Sum("missing".into()), &rows),
            Some(fmt_num_typed(0.0))
        );
        assert_eq!(
            compute_agg(&ctx, &Agg::Avg("v".into()), &rows),
            Some(fmt_num_typed(5.0 / 3.0))
        );
        assert_eq!(compute_agg(&ctx, &Agg::Avg("missing".into()), &rows), None);
        assert_eq!(
            compute_agg(&ctx, &Agg::Sample("text".into()), &rows),
            Some("\"z\"".into())
        );
        assert_eq!(
            compute_agg(&ctx, &Agg::Sample("missing".into()), &rows),
            None
        );
        assert_eq!(
            compute_agg(
                &ctx,
                &Agg::GroupConcat("text".into(), "|".into(), false),
                &rows
            ),
            Some("\"z|a|a\"".into())
        );
        assert_eq!(
            compute_agg(
                &ctx,
                &Agg::GroupConcat("text".into(), "|".into(), true),
                &rows
            ),
            Some("\"z|a\"".into())
        );
        assert_eq!(
            compute_agg(
                &ctx,
                &Agg::GroupConcat("missing".into(), "|".into(), true),
                &rows
            ),
            Some("\"\"".into())
        );
        assert_eq!(
            compute_agg(&ctx, &Agg::Min("v".into()), &rows),
            Some("1".into())
        );
        assert_eq!(
            compute_agg(&ctx, &Agg::Max("v".into()), &rows),
            Some("2".into())
        );
        assert_eq!(
            compute_agg(&ctx, &Agg::Min("text".into()), &rows),
            Some("\"a\"".into())
        );
        assert_eq!(
            compute_agg(&ctx, &Agg::Max("text".into()), &rows),
            Some("\"z\"".into())
        );
        assert_eq!(compute_agg(&ctx, &Agg::Min("missing".into()), &rows), None);

        let empty_bound = vec![row(&ctx, "\"a\"", None, None)];
        assert_eq!(
            compute_agg(&ctx, &Agg::Avg("v".into()), &empty_bound),
            Some(fmt_num_typed(0.0))
        );
        let invalid = vec![row(&ctx, "\"a\"", Some("\"oops\""), None)];
        assert_eq!(compute_agg(&ctx, &Agg::Avg("v".into()), &invalid), None);
    }

    #[test]
    fn grouping_preserves_keys_materializes_pre_expressions_and_emits_empty_group() {
        let rete = fixture();
        let ctx = context(&rete);
        let rows = vec![
            row(&ctx, "\"b\"", Some("2"), None),
            row(&ctx, "\"a\"", Some("1"), None),
            row(&ctx, "\"a\"", Some("3"), None),
        ];
        let spec = GroupSpec {
            by: vec!["g".into()],
            aggs: vec![("result".into(), Agg::Sum("pre".into()))],
            pre: vec![(
                "pre".into(),
                FExpr::Arith(
                    ArithOp::Mul,
                    Box::new(FExpr::Var("v".into())),
                    Box::new(FExpr::Const("2".into())),
                ),
            )],
        };
        let grouped = aggregate(&ctx, rows, &spec);
        assert_eq!(grouped.len(), 2);
        let g = ctx.slots.slot("g").unwrap();
        let result = ctx.slots.slot("result").unwrap();
        assert_eq!(
            &*ctx
                .resolver
                .str_of(grouped[0][g].as_ref().unwrap())
                .unwrap(),
            "\"a\""
        );
        assert_eq!(
            ctx.resolver.num(grouped[0][result].as_ref().unwrap()),
            Some(8.0)
        );
        assert_eq!(
            &*ctx
                .resolver
                .str_of(grouped[1][g].as_ref().unwrap())
                .unwrap(),
            "\"b\""
        );
        assert_eq!(
            ctx.resolver.num(grouped[1][result].as_ref().unwrap()),
            Some(4.0)
        );

        let empty = GroupSpec {
            by: vec![],
            aggs: vec![("result".into(), Agg::CountStar { distinct: true })],
            pre: vec![],
        };
        let grouped = aggregate(&ctx, Vec::new(), &empty);
        assert_eq!(grouped.len(), 1);
        assert_eq!(
            ctx.resolver.num(grouped[0][result].as_ref().unwrap()),
            Some(0.0)
        );
    }
}
