//! Plan evaluation: turn a lowered [`Select`]/[`Plan`] into solution rows.
//!
//! Evaluation is a lazy pull pipeline (volcano model): every algebra node in
//! [`eval_plan_iter`] yields an iterator of integer slot [`Row`]s, so `LIMIT`,
//! `ASK` and `DISTINCT … LIMIT` propagate demand all the way down to the index
//! scan and stop early. Blocking points are only what the semantics force:
//! aggregation, ORDER BY (top-k when a LIMIT bounds it), and the *build* side
//! of hash joins / MINUS — their probe sides stream. Terms are resolved to
//! strings only at the projection boundary (late materialization). Aggregates,
//! expressions and property paths live in the sibling `aggregate`/`expr`/`path`
//! modules.

use super::aggregate::aggregate;
use super::expr::SortKey;
use super::path::eval_path;
use super::*;
use crate::bgp::{
    bgp_exists, collect_pattern_slots, eval_bgp_rows, row_to_binding, BgpSolutions, Binding,
    PatternTerm, ProbeJoin, ProbePlan, TriplePattern,
};
use crate::file::Rete;
use crate::index::{GraphIndex, GraphIndexBuilder};
use crate::row::{bound_mask, merge_rows, Ctx, Row, Slots, Val};
use spargebra::term::{NamedNodePattern, TermPattern, TriplePattern as SpTriplePattern};

/// A lazily-evaluated stream of solution rows.
pub(super) type RowIter<'q> = Box<dyn Iterator<Item = Row> + 'q>;

/// The largest `limit_hint` for which joins switch from hash joins (scan every
/// pattern once) to index-nested-loop probing (probe per row). Above this, the
/// per-row probes would likely cost more than the one-pass scans they avoid.
const INLJ_MAX_HINT: usize = 4096;

/// The demand bound, when small enough to make index probing the better join
/// strategy.
fn inlj_hint(ctx: &Ctx) -> Option<usize> {
    ctx.limit_hint.get().filter(|&h| h <= INLJ_MAX_HINT)
}

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

// --- static binding analysis -------------------------------------------------
//
// Streaming joins need their hash key before any left row arrives, so the key
// is the slots a plan *always* binds (`certain`) rather than the slots its
// materialized rows happen to bind. A maybe-bound shared slot simply isn't part
// of the bucket key — `merge_rows`/`minus_compatible` still verify it per
// candidate, so the result is unchanged; the bucket is just less selective for
// those (rare, OPTIONAL/UNION-shaped) rows.

/// Slots bound in **every** row the plan can yield.
fn certain_bound(ctx: &Ctx, plan: &Plan, n: usize) -> Vec<bool> {
    let mut m = vec![false; n];
    mark_certain(ctx, plan, &mut m);
    m
}

fn mark_certain(ctx: &Ctx, plan: &Plan, m: &mut [bool]) {
    let mark_var = |v: &str, m: &mut [bool]| {
        if let Some(i) = ctx.slots.slot(v) {
            m[i] = true;
        }
    };
    match plan {
        Plan::Bgp(patterns) => {
            for p in patterns {
                for t in [&p.s, &p.p, &p.o] {
                    if let PatternTerm::Var(v) = t {
                        mark_var(v, m);
                    }
                }
            }
        }
        Plan::Path(s, _, o) => {
            for t in [s, o] {
                if let PatternTerm::Var(v) = t {
                    mark_var(v, m);
                }
            }
        }
        Plan::Values(vars, rows) => {
            for (vi, v) in vars.iter().enumerate() {
                if rows
                    .iter()
                    .all(|row| row.get(vi).is_some_and(Option::is_some))
                {
                    mark_var(v, m);
                }
            }
        }
        Plan::Filter(_, inner) => mark_certain(ctx, inner, m),
        Plan::Union(l, r) => {
            // Certain only when certain in *both* branches.
            let a = certain_bound(ctx, l, m.len());
            let b = certain_bound(ctx, r, m.len());
            for (i, slot) in m.iter_mut().enumerate() {
                *slot |= a[i] && b[i];
            }
        }
        Plan::Join(l, r) => {
            mark_certain(ctx, l, m);
            mark_certain(ctx, r, m);
        }
        Plan::LeftJoin(l, _, _) | Plan::Minus(l, _) => mark_certain(ctx, l, m),
        Plan::Graph(target, inner) => {
            if let GraphTarget::Var(v) = target {
                mark_var(v, m);
            }
            mark_certain(ctx, inner, m);
        }
    }
}

/// Slots bound in **some** row the plan could yield (over-approximation: every
/// variable the plan mentions).
fn possible_bound(ctx: &Ctx, plan: &Plan, n: usize) -> Vec<bool> {
    let mut m = vec![false; n];
    mark_possible(ctx, plan, &mut m);
    m
}

fn mark_possible(ctx: &Ctx, plan: &Plan, m: &mut [bool]) {
    let mark_var = |v: &str, m: &mut [bool]| {
        if let Some(i) = ctx.slots.slot(v) {
            m[i] = true;
        }
    };
    match plan {
        Plan::Bgp(patterns) => {
            for p in patterns {
                for t in [&p.s, &p.p, &p.o] {
                    if let PatternTerm::Var(v) = t {
                        mark_var(v, m);
                    }
                }
            }
        }
        Plan::Path(s, _, o) => {
            for t in [s, o] {
                if let PatternTerm::Var(v) = t {
                    mark_var(v, m);
                }
            }
        }
        Plan::Values(vars, _) => {
            for v in vars {
                mark_var(v, m);
            }
        }
        Plan::Filter(_, inner) => mark_possible(ctx, inner, m),
        Plan::Union(l, r) | Plan::Join(l, r) | Plan::LeftJoin(l, r, _) => {
            mark_possible(ctx, l, m);
            mark_possible(ctx, r, m);
        }
        Plan::Minus(l, _) => mark_possible(ctx, l, m),
        Plan::Graph(target, inner) => {
            if let GraphTarget::Var(v) = target {
                mark_var(v, m);
            }
            mark_possible(ctx, inner, m);
        }
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
    // ASK pulls exactly one solution — let joins probe instead of scan.
    ctx.limit_hint.set(Some(1));
    let merged = if sel.from.is_empty() {
        None
    } else {
        Some(merge_graphs(rete, &sel.from))
    };
    let active = merged.as_ref().unwrap_or_else(|| rete.default_index());
    plan_exists(&ctx, active, sel.from_named.as_deref(), &sel.plan)
}

/// Does `plan` have at least one solution against `index`? The single-pattern
/// BGP keeps its dedicated index probe; everything else pulls one row from the
/// lazy pipeline and stops.
fn plan_exists(ctx: &Ctx, index: &GraphIndex, nf: Option<&[String]>, plan: &Plan) -> bool {
    match plan {
        // The single-pattern probe is a direct index lookup; multi-pattern
        // BGPs go through the pipeline, which probes under ASK's demand bound.
        Plan::Bgp(patterns) if patterns.len() <= 1 => bgp_exists(ctx, index, patterns),
        Plan::Union(l, r) => plan_exists(ctx, index, nf, l) || plan_exists(ctx, index, nf, r),
        Plan::Values(_, rows) => !rows.is_empty(),
        _ => eval_plan_iter(ctx, index, nf, plan).next().is_some(),
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
        Some(g) => aggregate(ctx, eval_plan_iter(ctx, active, nf, &sel.plan).collect(), g),
        None => eval_plan_iter(ctx, active, nf, &sel.plan).collect(),
    };
    for row in raw.iter_mut() {
        apply_extends_row(ctx, row, &sel.extends);
    }
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

/// Apply BIND/alias assignments to one row (columns only — never drops rows).
fn apply_extends_row(ctx: &Ctx, row: &mut Row, extends: &[(String, FExpr)]) {
    for (var, expr) in extends.iter().rev() {
        if let Some(slot) = ctx.slots.slot(var) {
            if let Some(v) = expr.value(ctx, row) {
                row[slot] = Some(ctx.resolver.canon_term(&v));
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

/// Run a lowered SELECT as a lazy modifier pipeline over the plan iterator:
/// extends → HAVING → ORDER BY (top-k under LIMIT) → projection → DISTINCT →
/// slice → late materialization. Only ORDER BY and aggregation block; every
/// other stage streams, so the slice's demand reaches the index scan.
pub(super) fn run_select(rete: &Rete, sel: &Select) -> (Vec<String>, Vec<Binding>) {
    let ctx = query_ctx(rete, sel);
    // A pure LIMIT/OFFSET (no ORDER BY/DISTINCT/aggregate/HAVING, which all
    // consume their input fully) bounds how many rows the pipeline will pull —
    // joins below may switch to index probing. BIND only adds columns.
    if sel.order.is_empty() && !sel.distinct && sel.group.is_none() && sel.having.is_empty() {
        ctx.limit_hint
            .set(sel.limit.map(|l| l.saturating_add(sel.offset)));
    }
    let merged = if sel.from.is_empty() {
        None
    } else {
        Some(merge_graphs(rete, &sel.from))
    };
    let active = merged.as_ref().unwrap_or_else(|| rete.default_index());
    let nf = sel.from_named.as_deref();

    // Source rows: aggregation is blocking; everything else streams.
    let mut source: RowIter = match &sel.group {
        Some(g) => Box::new(
            aggregate(
                &ctx,
                eval_plan_iter(&ctx, active, nf, &sel.plan).collect(),
                g,
            )
            .into_iter(),
        ),
        None => eval_plan_iter(&ctx, active, nf, &sel.plan),
    };

    // BIND/aliases add columns per row — streaming.
    if !sel.extends.is_empty() {
        let ctx_ref = &ctx;
        let extends = &sel.extends;
        source = Box::new(source.map(move |mut row| {
            apply_extends_row(ctx_ref, &mut row, extends);
            row
        }));
    }

    // HAVING filters aggregated rows — streaming.
    if !sel.having.is_empty() {
        let ctx_ref = &ctx;
        let having = &sel.having;
        let mut cache = ExistsCache::new();
        source = Box::new(source.filter(move |b| {
            having
                .iter()
                .all(|f| f.boolean(ctx_ref, active, b, &mut cache))
        }));
    }

    // ORDER BY blocks, but with a LIMIT (and no DISTINCT, which would dedup
    // *after* the cut) only the top `offset + limit` rows are kept — O(n·k)
    // bounded insertion instead of a full sort.
    if !sel.order.is_empty() {
        let sorted = match (sel.limit, sel.distinct) {
            (Some(limit), false) => {
                top_k(&ctx, source, &sel.order, sel.offset.saturating_add(limit))
            }
            _ => sort_all(&ctx, source, &sel.order),
        };
        source = Box::new(sorted.into_iter());
    }

    // Project to the requested slots (SELECT * keeps everything). Only DISTINCT
    // needs the materialized projected row (its identity is the projection);
    // otherwise the final conversion below reads the projected slots straight
    // off the raw row — no per-row clone.
    let proj_slots: Vec<usize> = sel
        .project
        .iter()
        .filter_map(|v| ctx.slots.slot(v))
        .collect();
    if !sel.project.is_empty() && sel.distinct {
        let ctx_ref = &ctx;
        let ps = proj_slots.clone();
        source = Box::new(source.map(move |b| {
            let mut p = ctx_ref.slots.empty_row();
            for &slot in &ps {
                p[slot] = b[slot].clone();
            }
            p
        }));
    }

    // DISTINCT dedups on the integer rows — streaming, so DISTINCT … LIMIT
    // stops the scan as soon as enough distinct rows have surfaced.
    if sel.distinct {
        let mut seen: std::collections::HashSet<Row> = std::collections::HashSet::new();
        source = Box::new(source.filter(move |row| seen.insert(row.clone())));
    }

    // Slice, then resolve only the surviving rows' projected values.
    let rows: Vec<Binding> = source
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

/// Compare two decorated rows by the ORDER BY spec, with the arrival sequence
/// as the final tiebreak (= a stable sort's order).
fn cmp_keyed(
    order: &[(FExpr, bool)],
    a: &(Vec<SortKey>, usize, Row),
    b: &(Vec<SortKey>, usize, Row),
) -> std::cmp::Ordering {
    for (i, (_, desc)) in order.iter().enumerate() {
        let ord = a.0[i].cmp(&b.0[i]);
        let ord = if *desc { ord.reverse() } else { ord };
        if ord != std::cmp::Ordering::Equal {
            return ord;
        }
    }
    a.1.cmp(&b.1)
}

/// Decorate–sort–undecorate: resolve each row's sort keys *once* (numeric value
/// pre-parsed) instead of re-evaluating them on every comparison.
fn sort_all(ctx: &Ctx, rows: RowIter, order: &[(FExpr, bool)]) -> Vec<Row> {
    let mut keyed: Vec<(Vec<SortKey>, usize, Row)> = rows
        .enumerate()
        .map(|(seq, b)| {
            let keys = order
                .iter()
                .map(|(e, _)| SortKey::of(e.value(ctx, &b)))
                .collect();
            (keys, seq, b)
        })
        .collect();
    keyed.sort_by(|a, b| cmp_keyed(order, a, b));
    keyed.into_iter().map(|(_, _, b)| b).collect()
}

/// The first `k` rows of the stable sort order, via bounded insertion — O(n·k)
/// worst case with k = LIMIT + OFFSET (small), instead of sorting all n rows.
fn top_k(ctx: &Ctx, rows: RowIter, order: &[(FExpr, bool)], k: usize) -> Vec<Row> {
    if k == 0 {
        return Vec::new();
    }
    let mut top: Vec<(Vec<SortKey>, usize, Row)> = Vec::with_capacity(k + 1);
    for (seq, b) in rows.enumerate() {
        let keys: Vec<SortKey> = order
            .iter()
            .map(|(e, _)| SortKey::of(e.value(ctx, &b)))
            .collect();
        let entry = (keys, seq, b);
        if top.len() >= k && cmp_keyed(order, &entry, &top[k - 1]) != std::cmp::Ordering::Less {
            continue;
        }
        let pos =
            top.partition_point(|e| cmp_keyed(order, e, &entry) != std::cmp::Ordering::Greater);
        top.insert(pos, entry);
        top.truncate(k);
    }
    top.into_iter().map(|(_, _, b)| b).collect()
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

/// Evaluate a plan eagerly to a row vector (used by EXISTS, whose solutions are
/// cached and probed repeatedly). The demand bound is suspended — this consumes
/// everything, so hash joins beat per-row probing here.
pub(crate) fn eval_plan_in(
    ctx: &Ctx,
    index: &GraphIndex,
    named_filter: Option<&[String]>,
    plan: &Plan,
) -> Vec<Row> {
    let saved = ctx.limit_hint.replace(None);
    let rows = eval_plan_iter(ctx, index, named_filter, plan).collect();
    ctx.limit_hint.set(saved);
    rows
}

/// Lazily evaluate a plan against a specific graph `index` (the active graph).
/// `named_filter` (from `FROM NAMED`) restricts which graphs `GRAPH` may see.
pub(crate) fn eval_plan_iter<'q>(
    ctx: &'q Ctx<'q>,
    index: &'q GraphIndex,
    named_filter: Option<&'q [String]>,
    plan: &'q Plan,
) -> RowIter<'q> {
    // A named graph is visible unless FROM NAMED excludes it.
    let visible = move |name: &str| named_filter.is_none_or(|f| f.iter().any(|g| g == name));
    match plan {
        Plan::Bgp(patterns) => {
            // Under a small demand bound, probe the join pattern-by-pattern
            // through the index instead of scanning every pattern once.
            if inlj_hint(ctx).is_some() && patterns.len() >= 2 {
                return match ProbeJoin::new(ctx, index, patterns) {
                    Some(pj) => Box::new(pj),
                    None => Box::new(std::iter::empty()),
                };
            }
            Box::new(BgpSolutions::new(ctx, index, patterns))
        }
        Plan::Path(subj, spec, obj) => Box::new(eval_path(ctx, index, subj, spec, obj).into_iter()),
        Plan::Values(vars, rows) => Box::new(values_rows(ctx, vars, rows).into_iter()),
        Plan::Filter(expr, inner) => {
            let mut cache = ExistsCache::new();
            Box::new(
                eval_plan_iter(ctx, index, named_filter, inner)
                    .filter(move |b| expr.boolean(ctx, index, b, &mut cache)),
            )
        }
        Plan::Union(l, r) => Box::new(
            eval_plan_iter(ctx, index, named_filter, l).chain(eval_plan_iter(
                ctx,
                index,
                named_filter,
                r,
            )),
        ),
        Plan::Minus(l, r) => minus_iter(ctx, index, named_filter, l, r),
        Plan::Join(l, r) => {
            // VALUES-driven pushdown: substitute few ground rows into the BGP
            // scan instead of scanning the whole pattern and hash-joining.
            if let Some(v) = values_pushdown(ctx, index, l, r) {
                return Box::new(v.into_iter());
            }
            join_iter(ctx, index, named_filter, l, r, false, None)
        }
        Plan::LeftJoin(l, r, cond) => {
            join_iter(ctx, index, named_filter, l, r, true, cond.as_ref())
        }
        // GRAPH switches the active graph index (subject to FROM NAMED).
        Plan::Graph(GraphTarget::Named(iri), inner) => match ctx.rete.graph_index(iri) {
            Some(gi) if visible(iri) => eval_plan_iter(ctx, gi, named_filter, inner),
            _ => Box::new(std::iter::empty()),
        },
        Plan::Graph(GraphTarget::Var(var), inner) => {
            let Some(slot) = ctx.slots.slot(var) else {
                return Box::new(std::iter::empty());
            };
            Box::new(
                ctx.rete
                    .named_graphs()
                    .iter()
                    .filter(move |(name, _)| visible(name))
                    .flat_map(move |(name, gi)| {
                        let gval = ctx.resolver.canon_term(name);
                        eval_plan_iter(ctx, gi, named_filter, inner).filter_map(move |mut sol| {
                            match &sol[slot] {
                                Some(existing) if *existing != gval => None,
                                _ => {
                                    sol[slot] = Some(gval.clone());
                                    Some(sol)
                                }
                            }
                        })
                    }),
            )
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
/// the BGP's scan instead of materializing the whole BGP and hash-joining.
/// Returns `None` (use the hash join) when neither side is a pushable
/// VALUES/BGP pair.
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

/// `MINUS` as a streaming anti-join: the right side is materialized and indexed
/// by the slots the left side always binds; left rows then stream through a
/// filter, each checked against its bucket's candidates (plus the right rows
/// not fully bound on the key) with [`minus_compatible`].
fn minus_iter<'q>(
    ctx: &'q Ctx<'q>,
    index: &'q GraphIndex,
    nf: Option<&'q [String]>,
    l: &'q Plan,
    r: &'q Plan,
) -> RowIter<'q> {
    use std::collections::HashMap;
    let right: Vec<Row> = eval_plan_iter(ctx, index, nf, r).collect();
    if right.is_empty() {
        return eval_plan_iter(ctx, index, nf, l);
    }
    let n = ctx.slots.len();
    let rmask = bound_mask(&right, n);
    // Disjoint domains ⇒ MINUS eliminates nothing.
    let lposs = possible_bound(ctx, l, n);
    if !(0..n).any(|i| lposs[i] && rmask[i]) {
        return eval_plan_iter(ctx, index, nf, l);
    }
    let lcert = certain_bound(ctx, l, n);
    let jv: Vec<usize> = (0..n).filter(|&i| lcert[i] && rmask[i]).collect();
    // Right rows fully bound on the key slots are bucketed; the rest are
    // scanned per left row.
    let mut buckets: HashMap<Vec<Val>, Vec<usize>> = HashMap::new();
    let mut partial: Vec<usize> = Vec::new();
    for (i, row) in right.iter().enumerate() {
        match jv
            .iter()
            .map(|&s| row[s].clone())
            .collect::<Option<Vec<Val>>>()
        {
            Some(k) => buckets.entry(k).or_default().push(i),
            None => partial.push(i),
        }
    }
    let left = eval_plan_iter(ctx, index, nf, l);
    Box::new(left.filter(move |lb| {
        let eliminated = match jv
            .iter()
            .map(|&s| lb[s].clone())
            .collect::<Option<Vec<Val>>>()
        {
            Some(k) => {
                let in_bucket = buckets
                    .get(&k)
                    .is_some_and(|c| c.iter().any(|&i| minus_compatible(lb, &right[i])));
                in_bucket || partial.iter().any(|&i| minus_compatible(lb, &right[i]))
            }
            // Missing a key slot (heterogeneous left): check every right row.
            None => right.iter().any(|rb| minus_compatible(lb, rb)),
        };
        !eliminated
    }))
}

/// A streaming hash join: the right side is materialized into buckets keyed by
/// the slots both sides *always* bind; left rows are then pulled one at a time,
/// each probing its bucket — so a `LIMIT` above the join stops the left scan.
/// `optional = true` is a left join (OPTIONAL): a left row with no surviving
/// match is emitted unchanged, and `cond` (the OPTIONAL's filter) decides which
/// merges count as a match. `merge_rows` re-checks every shared slot, so
/// maybe-bound slots outside the bucket key stay exact.
struct JoinIter<'q> {
    ctx: &'q Ctx<'q>,
    index: &'q GraphIndex,
    left: RowIter<'q>,
    right: Vec<Row>,
    buckets: std::collections::HashMap<Vec<Val>, Vec<usize>>,
    /// Right rows not fully bound on `jv` — candidates for every left row.
    partial: Vec<usize>,
    jv: Vec<usize>,
    optional: bool,
    cond: Option<&'q FExpr>,
    cache: ExistsCache,
    cur_left: Option<Row>,
    candidates: Vec<usize>,
    ci: usize,
    matched: bool,
}

fn join_iter<'q>(
    ctx: &'q Ctx<'q>,
    index: &'q GraphIndex,
    nf: Option<&'q [String]>,
    l: &'q Plan,
    r: &'q Plan,
    optional: bool,
    cond: Option<&'q FExpr>,
) -> RowIter<'q> {
    // Under a small demand bound with a BGP right side, skip materializing it:
    // stream the left and probe the right's patterns per row through the index
    // (correlated pushdown). Same multiset as the hash join.
    if inlj_hint(ctx).is_some() {
        if let Plan::Bgp(patterns) = r {
            if !patterns.is_empty() {
                let lcert = certain_bound(ctx, l, ctx.slots.len());
                return match ProbePlan::new(ctx, patterns, &lcert) {
                    Some(plan) => Box::new(ProbedJoin {
                        ctx,
                        index,
                        left: eval_plan_iter(ctx, index, nf, l),
                        plan,
                        optional,
                        cond,
                        cache: ExistsCache::new(),
                        cur: None,
                    }),
                    // An unknown constant empties the right side for every row.
                    None if optional => eval_plan_iter(ctx, index, nf, l),
                    None => Box::new(std::iter::empty()),
                };
            }
        }
    }
    // Build the right side first: an empty build side short-circuits without
    // ever constructing (or scanning) the left side.
    let right: Vec<Row> = eval_plan_iter(ctx, index, nf, r).collect();
    if right.is_empty() {
        return if optional {
            eval_plan_iter(ctx, index, nf, l)
        } else {
            Box::new(std::iter::empty())
        };
    }
    let n = ctx.slots.len();
    let jv: Vec<usize> = {
        let lcert = certain_bound(ctx, l, n);
        let rcert = certain_bound(ctx, r, n);
        let rmask = bound_mask(&right, n);
        (0..n)
            .filter(|&i| lcert[i] && rcert[i] && rmask[i])
            .collect()
    };
    let mut buckets: std::collections::HashMap<Vec<Val>, Vec<usize>> =
        std::collections::HashMap::new();
    let mut partial: Vec<usize> = Vec::new();
    for (i, row) in right.iter().enumerate() {
        match jv
            .iter()
            .map(|&s| row[s].clone())
            .collect::<Option<Vec<Val>>>()
        {
            Some(k) => buckets.entry(k).or_default().push(i),
            None => partial.push(i),
        }
    }
    Box::new(JoinIter {
        ctx,
        index,
        left: eval_plan_iter(ctx, index, nf, l),
        right,
        buckets,
        partial,
        jv,
        optional,
        cond,
        cache: ExistsCache::new(),
        cur_left: None,
        candidates: Vec::new(),
        ci: 0,
        matched: false,
    })
}

/// A correlated index-nested-loop join: left rows stream, and each one probes
/// the right side's BGP through the index with its bound values substituted
/// ([`ProbeJoin::from_plan`]). `optional`/`cond` follow the OPTIONAL semantics
/// of [`JoinIter`]. Chosen over the hash join only under a small demand bound.
struct ProbedJoin<'q> {
    ctx: &'q Ctx<'q>,
    index: &'q GraphIndex,
    left: RowIter<'q>,
    plan: ProbePlan,
    optional: bool,
    cond: Option<&'q FExpr>,
    cache: ExistsCache,
    /// The current left row, its probe iterator, and whether a merge passed.
    cur: Option<(Row, ProbeJoin<'q>, bool)>,
}

impl Iterator for ProbedJoin<'_> {
    type Item = Row;

    fn next(&mut self) -> Option<Row> {
        loop {
            if let Some((_, probe, matched)) = &mut self.cur {
                for m in probe.by_ref() {
                    if self
                        .cond
                        .is_none_or(|f| f.boolean(self.ctx, self.index, &m, &mut self.cache))
                    {
                        *matched = true;
                        return Some(m);
                    }
                }
                let (l, _, matched) = self.cur.take().unwrap();
                if self.optional && !matched {
                    return Some(l);
                }
            }
            let l = self.left.next()?;
            let probe = ProbeJoin::from_plan(self.ctx, self.index, &self.plan, l.clone());
            self.cur = Some((l, probe, false));
        }
    }
}

impl Iterator for JoinIter<'_> {
    type Item = Row;

    fn next(&mut self) -> Option<Row> {
        loop {
            if let Some(left) = &self.cur_left {
                while self.ci < self.candidates.len() {
                    let ri = self.candidates[self.ci];
                    self.ci += 1;
                    if let Some(m) = merge_rows(left, &self.right[ri]) {
                        if self
                            .cond
                            .is_none_or(|f| f.boolean(self.ctx, self.index, &m, &mut self.cache))
                        {
                            self.matched = true;
                            return Some(m);
                        }
                    }
                }
                let l = self.cur_left.take().unwrap();
                if self.optional && !self.matched {
                    return Some(l);
                }
            }
            let l = self.left.next()?;
            self.candidates = match self
                .jv
                .iter()
                .map(|&s| l[s].clone())
                .collect::<Option<Vec<Val>>>()
            {
                Some(k) => {
                    let mut c = self.buckets.get(&k).cloned().unwrap_or_default();
                    c.extend_from_slice(&self.partial);
                    c
                }
                // The left row lacks a key slot: every right row is a candidate.
                None => (0..self.right.len()).collect(),
            };
            self.ci = 0;
            self.matched = false;
            self.cur_left = Some(l);
        }
    }
}
