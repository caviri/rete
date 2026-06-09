//! Plan evaluation: turn a lowered [`Select`]/[`Plan`] into solution rows.
//! `run_select` applies the SPARQL solution-modifier sequence; `eval_plan_in`
//! evaluates the algebra (BGP/joins/paths/graph) against the active graph; the
//! join helpers (`hash_join_solutions`, `minus_hash`, `values_pushdown`)
//! operate on integer slot [`Row`]s — terms are resolved to strings only at
//! the projection boundary (late materialization). Aggregates, expressions and
//! property paths live in the sibling `aggregate`/`expr`/`path` modules.

use super::aggregate::aggregate;
use super::expr::SortKey;
use super::path::eval_path;
use super::*;
use crate::bgp::{
    bgp_exists, collect_pattern_slots, eval_bgp_rows, row_to_binding, BgpSolutions, Binding,
    PatternTerm, TriplePattern,
};
use crate::file::Rete;
use crate::index::{GraphIndex, GraphIndexBuilder};
use crate::row::{bound_mask, merge_rows, Ctx, Row, Slots, Val};
use spargebra::term::{NamedNodePattern, TermPattern, TriplePattern as SpTriplePattern};

/// Build the per-query evaluation context: walk the whole query (plan,
/// EXISTS sub-plans, BIND targets, aggregates, projection) once and assign
/// every variable a slot.
pub(super) fn query_ctx<'a>(rete: &'a Rete, sel: &Select) -> Ctx<'a> {
    let mut slots = Slots::new();
    collect_plan_slots(&sel.plan, &mut slots);
    for (var, e) in &sel.extends {
        collect_expr_slots(e, &mut slots);
        slots.add(var);
    }
    if let Some(g) = &sel.group {
        for v in &g.by {
            slots.add(v);
        }
        for (res_var, agg) in &g.aggs {
            slots.add(res_var);
            match agg {
                Agg::CountStar { .. } => {}
                Agg::Count(v, _)
                | Agg::Sum(v)
                | Agg::Avg(v)
                | Agg::Min(v)
                | Agg::Max(v)
                | Agg::Sample(v)
                | Agg::GroupConcat(v, _) => {
                    slots.add(v);
                }
            }
        }
    }
    for h in &sel.having {
        collect_expr_slots(h, &mut slots);
    }
    for (e, _) in &sel.order {
        collect_expr_slots(e, &mut slots);
    }
    for v in &sel.project {
        slots.add(v);
    }
    Ctx::new(rete, slots)
}

fn collect_plan_slots(plan: &Plan, slots: &mut Slots) {
    match plan {
        Plan::Bgp(patterns) => collect_pattern_slots(patterns, slots),
        Plan::Join(l, r) | Plan::Union(l, r) | Plan::Minus(l, r) => {
            collect_plan_slots(l, slots);
            collect_plan_slots(r, slots);
        }
        Plan::LeftJoin(l, r, cond) => {
            collect_plan_slots(l, slots);
            collect_plan_slots(r, slots);
            if let Some(e) = cond {
                collect_expr_slots(e, slots);
            }
        }
        Plan::Filter(e, inner) => {
            collect_expr_slots(e, slots);
            collect_plan_slots(inner, slots);
        }
        Plan::Path(s, _, o) => {
            for t in [s, o] {
                if let PatternTerm::Var(v) = t {
                    slots.add(v);
                }
            }
        }
        Plan::Values(vars, _) => {
            for v in vars {
                slots.add(v);
            }
        }
        Plan::Graph(target, inner) => {
            if let GraphTarget::Var(v) = target {
                slots.add(v);
            }
            collect_plan_slots(inner, slots);
        }
    }
}

fn collect_expr_slots(e: &FExpr, slots: &mut Slots) {
    match e {
        FExpr::Var(v) | FExpr::Bound(v) => {
            slots.add(v);
        }
        FExpr::Const(_) => {}
        FExpr::Arith(_, l, r) | FExpr::Compare(_, l, r) | FExpr::And(l, r) | FExpr::Or(l, r) => {
            collect_expr_slots(l, slots);
            collect_expr_slots(r, slots);
        }
        FExpr::Not(inner) => collect_expr_slots(inner, slots),
        FExpr::Func(_, args) | FExpr::Coalesce(args) => {
            for a in args {
                collect_expr_slots(a, slots);
            }
        }
        FExpr::Exists(plan) => collect_plan_slots(plan, slots),
    }
}

/// Evaluate an `ASK`: does the query have any solution? Streams and stops at the
/// first solution for the common shapes; defers to the full evaluator only where
/// a solution's existence depends on aggregation/HAVING/post-aggregate aliases.
pub(super) fn ask_solution(rete: &Rete, sel: &Select) -> bool {
    let ctx = query_ctx(rete, sel);
    // A grouped query always yields at least one group (so ASK over it hinges on
    // HAVING); BIND aliases may be referenced by HAVING. These need the full
    // aggregate path — fall back to materializing.
    if sel.group.is_some() || !sel.having.is_empty() || !sel.extends.is_empty() {
        return !raw_solutions_in(&ctx, sel).is_empty();
    }
    let merged = if sel.from.is_empty() {
        None
    } else {
        Some(merge_graphs(rete, &sel.from))
    };
    let active = merged.as_ref().unwrap_or_else(|| rete.default_index());
    plan_exists(&ctx, active, sel.from_named.as_deref(), &sel.plan)
}

/// Does `plan` have at least one solution against `index`? Streams the
/// streamable shapes and stops at the first solution; falls back to eager
/// evaluation (testing non-emptiness) for shapes that need a full join/filter —
/// which still benefit from the now-lazy per-pattern scan.
fn plan_exists(ctx: &Ctx, index: &GraphIndex, nf: Option<&[String]>, plan: &Plan) -> bool {
    match plan {
        Plan::Bgp(patterns) => bgp_exists(ctx, index, patterns),
        Plan::Union(l, r) => plan_exists(ctx, index, nf, l) || plan_exists(ctx, index, nf, r),
        Plan::Values(_, rows) => !rows.is_empty(),
        _ => !eval_plan_in(ctx, index, nf, plan).is_empty(),
    }
}

/// The cap (OFFSET + LIMIT) for a pure-LIMIT early-out, or `None` if the query
/// has an ORDER BY / DISTINCT / aggregate / HAVING that must see the full
/// solution set first. BIND (`extends`) is allowed: it adds columns without
/// dropping rows, so it can be applied to the capped prefix afterwards.
fn early_out_cap(sel: &Select) -> Option<usize> {
    if sel.order.is_empty() && !sel.distinct && sel.group.is_none() && sel.having.is_empty() {
        sel.limit.map(|l| sel.offset.saturating_add(l))
    } else {
        None
    }
}

/// Evaluate `plan` to at most `cap` solutions, stopping early where the shape
/// allows (BGP join, FILTER-over-BGP, UNION, VALUES); other shapes fall back to
/// full evaluation truncated to `cap` (correct, just no early-out). Sound only
/// when the caller has no ORDER BY/DISTINCT/aggregate (see [`early_out_cap`]), so
/// any `cap`-sized prefix of solutions is a valid result.
fn eval_plan_capped(
    ctx: &Ctx,
    index: &GraphIndex,
    nf: Option<&[String]>,
    plan: &Plan,
    cap: usize,
) -> Vec<Row> {
    if cap == 0 {
        return Vec::new();
    }
    match plan {
        Plan::Bgp(patterns) => BgpSolutions::new(ctx, index, patterns).take(cap).collect(),
        // A FILTER over a BGP streams the join and keeps passing rows until `cap`.
        Plan::Filter(expr, inner) if matches!(inner.as_ref(), Plan::Bgp(_)) => {
            let Plan::Bgp(patterns) = inner.as_ref() else {
                unreachable!()
            };
            let mut out = Vec::new();
            let mut cache = ExistsCache::new();
            for b in BgpSolutions::new(ctx, index, patterns) {
                if expr.boolean(ctx, index, &b, &mut cache) {
                    out.push(b);
                    if out.len() >= cap {
                        break;
                    }
                }
            }
            out
        }
        Plan::Union(l, r) => {
            let mut out = eval_plan_capped(ctx, index, nf, l, cap);
            if out.len() < cap {
                let need = cap - out.len();
                out.extend(eval_plan_capped(ctx, index, nf, r, need));
            }
            out
        }
        Plan::Values(vars, rows) => values_rows(ctx, vars, &rows[..rows.len().min(cap)]),
        _ => eval_plan_in(ctx, index, nf, plan)
            .into_iter()
            .take(cap)
            .collect(),
    }
}

/// Raw solutions for a lowered pattern: plan + GROUP BY + aggregate aliases,
/// before projection/DISTINCT/slice (which are SELECT-specific). Returns the
/// evaluation context alongside the rows so callers can resolve terms.
pub(super) fn raw_solutions<'a>(rete: &'a Rete, sel: &Select) -> (Ctx<'a>, Vec<Row>) {
    let ctx = query_ctx(rete, sel);
    let rows = raw_solutions_in(&ctx, sel);
    (ctx, rows)
}

fn raw_solutions_in(ctx: &Ctx, sel: &Select) -> Vec<Row> {
    // The active default graph: `FROM` makes it the union of named graphs.
    let merged = if sel.from.is_empty() {
        None
    } else {
        Some(merge_graphs(ctx.rete, &sel.from))
    };
    let active = merged.as_ref().unwrap_or_else(|| ctx.rete.default_index());
    let nf = sel.from_named.as_deref();

    let mut raw = match &sel.group {
        // Grouping runs directly on the integer rows — only group keys and the
        // values an aggregate needs are ever resolved.
        Some(g) => aggregate(ctx, eval_plan_in(ctx, active, nf, &sel.plan), g),
        None => eval_plan_in(ctx, active, nf, &sel.plan),
    };
    apply_extends(ctx, &mut raw, &sel.extends);
    // HAVING runs on the aggregated (and aliased) rows.
    if !sel.having.is_empty() {
        let mut cache = ExistsCache::new();
        raw.retain(|b| {
            sel.having
                .iter()
                .all(|f| f.boolean(ctx, active, b, &mut cache))
        });
    }
    raw
}

/// Apply BIND/alias assignments to each row (columns only — never drops rows).
fn apply_extends(ctx: &Ctx, rows: &mut [Row], extends: &[(String, FExpr)]) {
    if extends.is_empty() {
        return;
    }
    let targets: Vec<Option<usize>> = extends.iter().map(|(v, _)| ctx.slots.slot(v)).collect();
    for row in rows.iter_mut() {
        for ((_, expr), slot) in extends.iter().zip(&targets).rev() {
            if let (Some(slot), Some(v)) = (slot, expr.value(ctx, row)) {
                row[*slot] = Some(ctx.resolver.canon_term(&v));
            }
        }
    }
}

/// Build the RDF merge (union of triples) of the given named graphs as a single
/// index. All graphs share the dataset dictionary, so integer triples combine
/// directly. Missing graphs contribute nothing.
fn merge_graphs(rete: &Rete, graphs: &[String]) -> GraphIndex {
    let mut b = GraphIndexBuilder::new();
    for g in graphs {
        if let Some(gi) = rete.graph_index(g) {
            for t in gi.match_pattern((None, None, None)) {
                b.push(t);
            }
        }
    }
    b.build()
}

/// Instantiate a CONSTRUCT template against solutions (triples with any unbound
/// variable are dropped; the result is deduplicated).
pub(super) fn instantiate(
    ctx: &Ctx,
    template: &[SpTriplePattern],
    sols: &[Row],
) -> Vec<(String, String, String)> {
    let mut set = std::collections::BTreeSet::new();
    for b in sols {
        for tp in template {
            if let (Some(s), Some(p), Some(o)) = (
                inst_term(ctx, &tp.subject, b),
                inst_named(ctx, &tp.predicate, b),
                inst_term(ctx, &tp.object, b),
            ) {
                set.insert((s, p, o));
            }
        }
    }
    set.into_iter().collect()
}

fn row_var(ctx: &Ctx, name: &str, b: &Row) -> Option<String> {
    let slot = ctx.slots.slot(name)?;
    b[slot]
        .as_ref()
        .and_then(|v| ctx.resolver.str_of(v))
        .map(|t| t.to_string())
}

fn inst_term(ctx: &Ctx, t: &TermPattern, b: &Row) -> Option<String> {
    match t {
        TermPattern::NamedNode(n) => Some(n.to_string()),
        TermPattern::Literal(l) => Some(l.to_string()),
        TermPattern::BlankNode(bn) => Some(bn.to_string()),
        TermPattern::Variable(v) => row_var(ctx, v.as_str(), b),
    }
}

fn inst_named(ctx: &Ctx, n: &NamedNodePattern, b: &Row) -> Option<String> {
    match n {
        NamedNodePattern::NamedNode(nn) => Some(nn.to_string()),
        NamedNodePattern::Variable(v) => row_var(ctx, v.as_str(), b),
    }
}

/// Run a lowered SELECT: raw solutions, ORDER BY, then projection, DISTINCT,
/// and slice (the SPARQL solution-modifier sequence). Rows stay integer slot
/// rows until after DISTINCT and the slice — only the surviving rows' projected
/// values are resolved to terms.
pub(super) fn run_select(rete: &Rete, sel: &Select) -> (Vec<String>, Vec<Binding>) {
    let ctx = query_ctx(rete, sel);

    let mut raw = match early_out_cap(sel) {
        // Pure LIMIT/OFFSET with no ORDER BY/DISTINCT/aggregate: produce only the
        // rows we need and stop. LIMIT without ORDER BY may return any subset, so
        // a streamed prefix of solutions is spec-compliant.
        Some(cap) => {
            let merged = if sel.from.is_empty() {
                None
            } else {
                Some(merge_graphs(rete, &sel.from))
            };
            let active = merged.as_ref().unwrap_or_else(|| rete.default_index());
            let mut rows =
                eval_plan_capped(&ctx, active, sel.from_named.as_deref(), &sel.plan, cap);
            // BIND (extends) add columns without dropping rows — apply post-cap.
            apply_extends(&ctx, &mut rows, &sel.extends);
            rows
        }
        None => raw_solutions_in(&ctx, sel),
    };

    // ORDER BY runs before projection so it can see unprojected variables.
    // Decorate–sort–undecorate: resolve each row's sort keys *once* (with the
    // numeric value pre-parsed) instead of re-evaluating + re-parsing them on
    // every comparison — O(n) key builds vs. O(n log n) in `sort_by`.
    if !sel.order.is_empty() {
        let mut keyed: Vec<(Vec<SortKey>, Row)> = raw
            .into_iter()
            .map(|b| {
                let keys = sel
                    .order
                    .iter()
                    .map(|(e, _)| SortKey::of(e.value(&ctx, &b)))
                    .collect();
                (keys, b)
            })
            .collect();
        keyed.sort_by(|(ka, _), (kb, _)| {
            for (i, (_, desc)) in sel.order.iter().enumerate() {
                let ord = ka[i].cmp(&kb[i]);
                let ord = if *desc { ord.reverse() } else { ord };
                if ord != std::cmp::Ordering::Equal {
                    return ord;
                }
            }
            std::cmp::Ordering::Equal
        });
        raw = keyed.into_iter().map(|(_, b)| b).collect();
    }

    // Project to the requested slots (SELECT * keeps everything). Rows stay
    // integer-valued; unprojected slots are simply cleared so DISTINCT sees
    // only the projected identity.
    let proj_slots: Vec<usize> = sel
        .project
        .iter()
        .filter_map(|v| ctx.slots.slot(v))
        .collect();
    let mut rows: Vec<Row> = if sel.project.is_empty() {
        raw
    } else {
        raw.into_iter()
            .map(|b| {
                let mut p = ctx.slots.empty_row();
                for &slot in &proj_slots {
                    p[slot] = b[slot].clone();
                }
                p
            })
            .collect()
    };

    if sel.distinct {
        // Dedup on the integer rows directly — no term resolution, no string
        // keys; only the survivors are ever materialized below.
        let mut seen: std::collections::HashSet<Row> = std::collections::HashSet::new();
        rows.retain(|row| seen.insert(row.clone()));
    }

    let rows: Vec<Binding> = rows
        .into_iter()
        .skip(sel.offset)
        .take(sel.limit.unwrap_or(usize::MAX))
        .map(|row| {
            if sel.project.is_empty() {
                row_to_binding(&ctx, &row)
            } else {
                let mut b = Binding::new();
                for (v, &slot) in sel.project.iter().zip(&proj_slots) {
                    if let Some(val) = &row[slot] {
                        if let Some(t) = ctx.resolver.str_once(val) {
                            b.insert(v.clone(), t);
                        }
                    }
                }
                b
            }
        })
        .collect();

    (sel.project.clone(), rows)
}

/// Inline `VALUES` rows as slot rows (tokens canonicalized to dictionary ids
/// where they exist, so they join exactly like scanned values).
fn values_rows(ctx: &Ctx, vars: &[String], rows: &[Vec<Option<String>>]) -> Vec<Row> {
    let slots: Vec<Option<usize>> = vars.iter().map(|v| ctx.slots.slot(v)).collect();
    rows.iter()
        .map(|row| {
            let mut r = ctx.slots.empty_row();
            for (slot, val) in slots.iter().zip(row.iter()) {
                if let (Some(i), Some(t)) = (slot, val) {
                    r[*i] = Some(ctx.resolver.canon_term(t));
                }
            }
            r
        })
        .collect()
}

/// Evaluate a plan against a specific graph `index` (the active graph).
/// `named_filter` (from `FROM NAMED`) restricts which graphs `GRAPH` may see.
pub(crate) fn eval_plan_in(
    ctx: &Ctx,
    index: &GraphIndex,
    named_filter: Option<&[String]>,
    plan: &Plan,
) -> Vec<Row> {
    let recur = |p: &Plan| eval_plan_in(ctx, index, named_filter, p);
    // A named graph is visible unless FROM NAMED excludes it.
    let visible = |name: &str| named_filter.is_none_or(|f| f.iter().any(|g| g == name));
    match plan {
        Plan::Bgp(patterns) => eval_bgp_rows(ctx, index, patterns),
        Plan::Path(subj, spec, obj) => eval_path(ctx, index, subj, spec, obj),
        Plan::Values(vars, rows) => values_rows(ctx, vars, rows),
        Plan::Filter(expr, inner) => {
            let mut v = recur(inner);
            let mut cache = ExistsCache::new();
            v.retain(|b| expr.boolean(ctx, index, b, &mut cache));
            v
        }
        Plan::Union(l, r) => {
            let mut v = recur(l);
            v.extend(recur(r));
            v
        }
        Plan::Minus(l, r) => minus_hash(ctx, recur(l), recur(r)),
        Plan::Join(l, r) => values_pushdown(ctx, index, l, r)
            .unwrap_or_else(|| hash_join_solutions(ctx, index, recur(l), recur(r), false, None)),
        Plan::LeftJoin(l, r, cond) => {
            hash_join_solutions(ctx, index, recur(l), recur(r), true, cond.as_ref())
        }
        // GRAPH switches the active graph index (subject to FROM NAMED).
        Plan::Graph(GraphTarget::Named(iri), inner) => match ctx.rete.graph_index(iri) {
            Some(gi) if visible(iri) => eval_plan_in(ctx, gi, named_filter, inner),
            _ => Vec::new(),
        },
        Plan::Graph(GraphTarget::Var(var), inner) => {
            let Some(slot) = ctx.slots.slot(var) else {
                return Vec::new();
            };
            let mut out = Vec::new();
            for (name, gi) in ctx.rete.named_graphs() {
                if !visible(name) {
                    continue;
                }
                let gval = ctx.resolver.canon_term(name);
                for mut sol in eval_plan_in(ctx, gi, named_filter, inner) {
                    match &sol[slot] {
                        Some(existing) if *existing != gval => continue,
                        _ => {
                            sol[slot] = Some(gval.clone());
                        }
                    }
                    out.push(sol);
                }
            }
            out
        }
    }
}

/// Substitute bound variables into a BGP's patterns, turning each bound
/// variable into a constant term so the index scan can constrain on it.
fn substitute_patterns(
    patterns: &[TriplePattern],
    input: &[(String, String)],
) -> Vec<TriplePattern> {
    let sub = |t: &PatternTerm| -> PatternTerm {
        match t {
            PatternTerm::Var(v) => match input.iter().find(|(k, _)| k == v) {
                Some((_, val)) => PatternTerm::Const(val.clone()),
                None => t.clone(),
            },
            PatternTerm::Const(_) => t.clone(),
        }
    };
    patterns
        .iter()
        .map(|p| TriplePattern {
            s: sub(&p.s),
            p: sub(&p.p),
            o: sub(&p.o),
        })
        .collect()
}

/// `VALUES`-driven join pushdown: when one side of a join is inline `VALUES`
/// (few, ground rows) and the other is a BGP, substitute each VALUES row into
/// the BGP's scan instead of materializing the whole BGP and hash-joining. This
/// turns `VALUES ?d {…} ?p :discipline ?d` into a couple of selective scans
/// rather than one full-predicate scan filtered down. Returns `None` (use the
/// hash join) when neither side is a pushable VALUES/BGP pair.
fn values_pushdown(ctx: &Ctx, index: &GraphIndex, l: &Plan, r: &Plan) -> Option<Vec<Row>> {
    let (vals, patterns) = match (l, r) {
        (Plan::Values(v, rows), Plan::Bgp(p)) | (Plan::Bgp(p), Plan::Values(v, rows)) => {
            ((v, rows), p)
        }
        _ => return None,
    };
    let (vars, rows) = vals;
    // Only beneficial when a VALUES variable actually appears in the BGP (so the
    // substitution constrains the scan); a disjoint pair is a Cartesian product
    // better handled once by the hash join than re-scanned per VALUES row.
    let bgp_vars: std::collections::HashSet<&str> = patterns
        .iter()
        .flat_map(|p| [&p.s, &p.p, &p.o])
        .filter_map(|t| match t {
            PatternTerm::Var(v) => Some(v.as_str()),
            PatternTerm::Const(_) => None,
        })
        .collect();
    if !vars.iter().any(|v| bgp_vars.contains(v.as_str())) {
        return None;
    }
    let mut out = Vec::new();
    for row in rows {
        // The bound variables from this VALUES row (UNDEF entries stay variable).
        let input: Vec<(String, String)> = vars
            .iter()
            .zip(row.iter())
            .filter_map(|(v, val)| val.as_ref().map(|t| (v.clone(), t.clone())))
            .collect();
        let subst = substitute_patterns(patterns, &input);
        // Re-attach this row's VALUES bindings (the substituted vars no longer
        // appear in the BGP result), then the BGP's own bindings.
        let mut base = ctx.slots.empty_row();
        for (v, t) in &input {
            if let Some(i) = ctx.slots.slot(v) {
                base[i] = Some(ctx.resolver.canon_term(t));
            }
        }
        for brow in eval_bgp_rows(ctx, index, &subst) {
            let mut merged = base.clone();
            for (slot, v) in brow.iter().enumerate() {
                if v.is_some() {
                    merged[slot] = v.clone();
                }
            }
            out.push(merged);
        }
    }
    Some(out)
}

/// Is a left row eliminated by a right row under `MINUS`? True iff they share at
/// least one bound slot and agree on every shared slot (SPARQL `MINUS`:
/// disjoint-domain rows never eliminate, and a disagreement keeps the left row).
fn minus_compatible(lb: &Row, rb: &Row) -> bool {
    let mut shared = false;
    for (l, r) in lb.iter().zip(rb.iter()) {
        if let (Some(v), Some(w)) = (l, r) {
            if v != w {
                return false;
            }
            shared = true;
        }
    }
    shared
}

/// `MINUS`: keep each left row unless some right row is [`minus_compatible`] with
/// it. Replaces the O(L×R) nested loop with an O(L + R) hash anti-join in the
/// common case (both sides bind the shared slots in every row): the shared
/// slots `jv` index the right rows, so a fully-bound left row is eliminated
/// by a single set lookup. Rows missing a shared slot (only via a nested
/// OPTIONAL/UNION) fall back to a scan, preserving exact semantics.
fn minus_hash(ctx: &Ctx, left: Vec<Row>, right: Vec<Row>) -> Vec<Row> {
    use std::collections::HashSet;
    if left.is_empty() || right.is_empty() {
        return left;
    }
    let n = ctx.slots.len();
    let lmask = bound_mask(&left, n);
    let rmask = bound_mask(&right, n);
    // Shared slots: those bound in some left row AND some right row.
    let jv: Vec<usize> = (0..n).filter(|&i| lmask[i] && rmask[i]).collect();
    if jv.is_empty() {
        // Disjoint domains ⇒ MINUS eliminates nothing.
        return left;
    }
    let key_of = |b: &Row| -> Option<Vec<Val>> { jv.iter().map(|&i| b[i].clone()).collect() };
    // Right rows fully bound on the shared slots index a set; the rest must be
    // scanned (a left row could share only a sub-domain with them).
    let mut full: HashSet<Vec<Val>> = HashSet::new();
    let mut partial_right: Vec<&Row> = Vec::new();
    for r in &right {
        match key_of(r) {
            Some(k) => {
                full.insert(k);
            }
            None => partial_right.push(r),
        }
    }
    left.iter()
        .filter(|lb| {
            match key_of(lb) {
                // Fully bound on the shared slots: a fully-bound right row
                // eliminates it iff their keys match; otherwise only a partial
                // right row could.
                Some(k) => {
                    if full.contains(&k) {
                        return false;
                    }
                    !partial_right.iter().any(|rb| minus_compatible(lb, rb))
                }
                // Missing a shared slot: must check every right row.
                None => !right.iter().any(|rb| minus_compatible(lb, rb)),
            }
        })
        .cloned()
        .collect()
}

/// Hash join two solution sets on the slots they share, emitting every
/// compatible merge. `optional = true` is a left join (OPTIONAL): a left row
/// with no surviving match is emitted unchanged, and `cond` (the OPTIONAL's
/// filter) decides which merges count as a match.
///
/// This replaces the O(L×R) nested-loop merge with an O(L + R + matches) hash
/// join in the common case where the join slots are bound in every row.
/// Rows missing a join slot (only possible via a nested OPTIONAL) fall back
/// to being tried against all candidates, preserving exact SPARQL semantics —
/// `merge_rows` still does the final compatibility check on every shared slot.
fn hash_join_solutions(
    ctx: &Ctx,
    index: &GraphIndex,
    left: Vec<Row>,
    right: Vec<Row>,
    optional: bool,
    cond: Option<&FExpr>,
) -> Vec<Row> {
    use std::collections::HashMap;
    if left.is_empty() {
        return Vec::new();
    }
    if right.is_empty() {
        return if optional { left } else { Vec::new() };
    }
    let n = ctx.slots.len();
    let lmask = bound_mask(&left, n);
    let rmask = bound_mask(&right, n);
    // Join slots: those bound on both sides.
    let jv: Vec<usize> = (0..n).filter(|&i| lmask[i] && rmask[i]).collect();

    // Key = the join-slot values, when all are bound; `None` ⇒ a join slot is
    // unbound in this row (a partial that must be matched against everything).
    let key_of = |b: &Row| -> Option<Vec<Val>> { jv.iter().map(|&i| b[i].clone()).collect() };
    let mut buckets: HashMap<Vec<Val>, Vec<usize>> = HashMap::new();
    let mut partial: Vec<usize> = Vec::new();
    for (i, r) in right.iter().enumerate() {
        match key_of(r) {
            Some(k) => buckets.entry(k).or_default().push(i),
            None => partial.push(i),
        }
    }

    let mut out = Vec::new();
    let mut cache = ExistsCache::new();
    let mut emit = |a: &Row, r: &Row, out: &mut Vec<Row>, matched: &mut bool| {
        if let Some(m) = merge_rows(a, r) {
            if cond.is_none_or(|f| f.boolean(ctx, index, &m, &mut cache)) {
                out.push(m);
                *matched = true;
            }
        }
    };
    for a in &left {
        let mut matched = false;
        match key_of(a) {
            Some(k) => {
                if let Some(idxs) = buckets.get(&k) {
                    for &i in idxs {
                        emit(a, &right[i], &mut out, &mut matched);
                    }
                }
                for &i in &partial {
                    emit(a, &right[i], &mut out, &mut matched);
                }
            }
            // `a` itself lacks a join slot: every right row is a candidate.
            None => {
                for r in &right {
                    emit(a, r, &mut out, &mut matched);
                }
            }
        }
        if optional && !matched {
            out.push(a.clone());
        }
    }
    out
}
