//! Plan evaluation: turn a lowered [`Select`]/[`Plan`] into solution bindings.
//! `run_select` applies the SPARQL solution-modifier sequence; `eval_plan_in`
//! evaluates the algebra (BGP/joins/paths/graph) against the active graph; the
//! join helpers (`hash_join_solutions`, `minus_hash`, `values_pushdown`) operate
//! on the resolved `Binding` rows. Aggregates, expressions and property paths
//! live in the sibling `aggregate`/`expr`/`path` modules.

use super::aggregate::{aggregate, aggregate_int};
use super::expr::SortKey;
use super::path::eval_path;
use super::*;
use crate::bgp::{eval_bgp_int_in, term_of_value, Binding, PatternTerm, TriplePattern};
use crate::file::Rete;
use crate::index::{GraphIndex, GraphIndexBuilder};
use spargebra::term::{NamedNodePattern, TermPattern, TriplePattern as SpTriplePattern};

/// Evaluate an `ASK`: does the query have any solution? Streams and stops at the
/// first solution for the common shapes; defers to the full evaluator only where
/// a solution's existence depends on aggregation/HAVING/post-aggregate aliases.
pub(super) fn ask_solution(rete: &Rete, sel: &Select) -> bool {
    // A grouped query always yields at least one group (so ASK over it hinges on
    // HAVING); BIND aliases may be referenced by HAVING. These need the full
    // aggregate path — fall back to materializing.
    if sel.group.is_some() || !sel.having.is_empty() || !sel.extends.is_empty() {
        return !raw_solutions(rete, sel).is_empty();
    }
    let merged = if sel.from.is_empty() {
        None
    } else {
        Some(merge_graphs(rete, &sel.from))
    };
    let active = merged.as_ref().unwrap_or_else(|| rete.default_index());
    plan_exists(rete, active, sel.from_named.as_deref(), &sel.plan)
}

/// Does `plan` have at least one solution against `index`? Streams the
/// streamable shapes and stops at the first solution; falls back to eager
/// evaluation (testing non-emptiness) for shapes that need a full join/filter —
/// which still benefit from the now-lazy per-pattern scan.
fn plan_exists(rete: &Rete, index: &GraphIndex, nf: Option<&[String]>, plan: &Plan) -> bool {
    match plan {
        Plan::Bgp(patterns) => crate::bgp::bgp_exists(rete, index, patterns),
        Plan::Union(l, r) => plan_exists(rete, index, nf, l) || plan_exists(rete, index, nf, r),
        Plan::Values(_, rows) => !rows.is_empty(),
        _ => !eval_plan_in(rete, index, nf, plan).is_empty(),
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
    rete: &Rete,
    index: &GraphIndex,
    nf: Option<&[String]>,
    plan: &Plan,
    cap: usize,
) -> Vec<Binding> {
    if cap == 0 {
        return Vec::new();
    }
    match plan {
        Plan::Bgp(patterns) => crate::bgp::BgpSolutions::new(rete, index, patterns)
            .take(cap)
            .collect(),
        // A FILTER over a BGP streams the join and keeps passing rows until `cap`.
        Plan::Filter(expr, inner) if matches!(inner.as_ref(), Plan::Bgp(_)) => {
            let Plan::Bgp(patterns) = inner.as_ref() else {
                unreachable!()
            };
            let mut out = Vec::new();
            let mut cache = ExistsCache::new();
            for b in crate::bgp::BgpSolutions::new(rete, index, patterns) {
                if expr.boolean(rete, index, &b, &mut cache) {
                    out.push(b);
                    if out.len() >= cap {
                        break;
                    }
                }
            }
            out
        }
        Plan::Union(l, r) => {
            let mut out = eval_plan_capped(rete, index, nf, l, cap);
            if out.len() < cap {
                let need = cap - out.len();
                out.extend(eval_plan_capped(rete, index, nf, r, need));
            }
            out
        }
        Plan::Values(vars, rows) => rows
            .iter()
            .take(cap)
            .map(|row| {
                vars.iter()
                    .zip(row.iter())
                    .filter_map(|(v, val)| val.as_ref().map(|t| (v.clone(), t.clone())))
                    .collect()
            })
            .collect(),
        _ => eval_plan_in(rete, index, nf, plan)
            .into_iter()
            .take(cap)
            .collect(),
    }
}

/// Fast path for `SELECT DISTINCT ?vars WHERE { BGP }`: dedup on the *integer*
/// bindings and resolve only the survivors to terms. When a distinct projection
/// collapses many matched rows to a few values (e.g. `DISTINCT ?discipline` over
/// every paper), this skips a term resolution + `Binding` allocation per matched
/// row — only the distinct projections are ever resolved. Applies OFFSET/LIMIT
/// after dedup (the caller guarantees no ORDER BY/GROUP/HAVING/BIND).
fn distinct_bgp_fast(
    rete: &Rete,
    sel: &Select,
    patterns: &[TriplePattern],
) -> (Vec<String>, Vec<Binding>) {
    let merged = if sel.from.is_empty() {
        None
    } else {
        Some(merge_graphs(rete, &sel.from))
    };
    let active = merged.as_ref().unwrap_or_else(|| rete.default_index());
    let dict = rete.dictionary();

    let mut seen: std::collections::HashSet<Vec<Option<i64>>> = std::collections::HashSet::new();
    let mut rows: Vec<Binding> = Vec::new();
    for ib in &eval_bgp_int_in(rete, active, patterns) {
        // Dedup key over the projected variables (an unprojected/unbound var is
        // `None`, so its absence is part of the distinct identity).
        let key: Vec<Option<i64>> = sel.project.iter().map(|v| ib.get(v).copied()).collect();
        if !seen.insert(key) {
            continue;
        }
        let mut b = Binding::new();
        for v in &sel.project {
            if let Some(&val) = ib.get(v) {
                if let Some(t) = term_of_value(dict, val) {
                    b.insert(v.clone(), t);
                }
            }
        }
        rows.push(b);
    }
    let rows = rows
        .into_iter()
        .skip(sel.offset)
        .take(sel.limit.unwrap_or(usize::MAX))
        .collect();
    (sel.project.clone(), rows)
}

/// Raw solutions for a lowered pattern: plan + GROUP BY + aggregate aliases,
/// before projection/DISTINCT/slice (which are SELECT-specific).
pub(super) fn raw_solutions(rete: &Rete, sel: &Select) -> Vec<Binding> {
    // The active default graph: `FROM` makes it the union of named graphs.
    let merged = if sel.from.is_empty() {
        None
    } else {
        Some(merge_graphs(rete, &sel.from))
    };
    let active = merged.as_ref().unwrap_or_else(|| rete.default_index());
    let nf = sel.from_named.as_deref();

    let mut raw = match (&sel.group, &sel.plan) {
        // Fast path: GROUP BY over a single BGP aggregates on integer bindings,
        // resolving only the group keys to terms (not every matched row).
        (Some(g), Plan::Bgp(patterns)) => {
            aggregate_int(rete, eval_bgp_int_in(rete, active, patterns), g)
        }
        (Some(g), _) => aggregate(eval_plan_in(rete, active, nf, &sel.plan), g),
        (None, _) => eval_plan_in(rete, active, nf, &sel.plan),
    };
    if !sel.extends.is_empty() {
        for row in &mut raw {
            for (var, expr) in sel.extends.iter().rev() {
                if let Some(v) = expr.value(row) {
                    row.insert(var.clone(), v);
                }
            }
        }
    }
    // HAVING runs on the aggregated (and aliased) rows.
    if !sel.having.is_empty() {
        let mut cache = ExistsCache::new();
        raw.retain(|b| {
            sel.having
                .iter()
                .all(|f| f.boolean(rete, active, b, &mut cache))
        });
    }
    raw
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
    template: &[SpTriplePattern],
    sols: &[Binding],
) -> Vec<(String, String, String)> {
    let mut set = std::collections::BTreeSet::new();
    for b in sols {
        for tp in template {
            if let (Some(s), Some(p), Some(o)) = (
                inst_term(&tp.subject, b),
                inst_named(&tp.predicate, b),
                inst_term(&tp.object, b),
            ) {
                set.insert((s, p, o));
            }
        }
    }
    set.into_iter().collect()
}

fn inst_term(t: &TermPattern, b: &Binding) -> Option<String> {
    match t {
        TermPattern::NamedNode(n) => Some(n.to_string()),
        TermPattern::Literal(l) => Some(l.to_string()),
        TermPattern::BlankNode(bn) => Some(bn.to_string()),
        TermPattern::Variable(v) => b.get(v.as_str()).cloned(),
    }
}

fn inst_named(n: &NamedNodePattern, b: &Binding) -> Option<String> {
    match n {
        NamedNodePattern::NamedNode(nn) => Some(nn.to_string()),
        NamedNodePattern::Variable(v) => b.get(v.as_str()).cloned(),
    }
}

/// Run a lowered SELECT: raw solutions, ORDER BY, then projection, DISTINCT,
/// and slice (the SPARQL solution-modifier sequence).
pub(super) fn run_select(rete: &Rete, sel: &Select) -> (Vec<String>, Vec<Binding>) {
    // Fast path: `SELECT DISTINCT ?vars WHERE { BGP }` (no ORDER BY/GROUP/HAVING/
    // BIND) dedups on integer bindings and resolves only the survivors.
    if sel.distinct
        && sel.group.is_none()
        && sel.having.is_empty()
        && sel.order.is_empty()
        && sel.extends.is_empty()
        && !sel.project.is_empty()
    {
        if let Plan::Bgp(patterns) = &sel.plan {
            return distinct_bgp_fast(rete, sel, patterns);
        }
    }

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
                eval_plan_capped(rete, active, sel.from_named.as_deref(), &sel.plan, cap);
            // BIND (extends) add columns without dropping rows — apply post-cap.
            for row in &mut rows {
                for (var, expr) in sel.extends.iter().rev() {
                    if let Some(v) = expr.value(row) {
                        row.insert(var.clone(), v);
                    }
                }
            }
            rows
        }
        None => raw_solutions(rete, sel),
    };

    // ORDER BY runs before projection so it can see unprojected variables.
    // Decorate–sort–undecorate: resolve each row's sort keys *once* (with the
    // numeric value pre-parsed) instead of re-evaluating + re-parsing them on
    // every comparison — O(n) key builds vs. O(n log n) in `sort_by`.
    if !sel.order.is_empty() {
        let mut keyed: Vec<(Vec<SortKey>, Binding)> = raw
            .into_iter()
            .map(|b| {
                let keys = sel
                    .order
                    .iter()
                    .map(|(e, _)| SortKey::of(e.value(&b)))
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

    // Project to the requested variables (SELECT * keeps everything).
    let mut rows: Vec<Binding> = raw
        .into_iter()
        .map(|b| {
            if sel.project.is_empty() {
                b
            } else {
                sel.project
                    .iter()
                    .filter_map(|v| b.get(v).map(|val| (v.clone(), val.clone())))
                    .collect()
            }
        })
        .collect();

    if sel.distinct {
        // Hash the row's (sorted) key/value pairs directly — `Binding` iterates
        // in key order, so the tuple vector is a canonical DISTINCT key without
        // the per-row `format!` Debug round-trip.
        let mut seen: std::collections::HashSet<Vec<(String, String)>> =
            std::collections::HashSet::new();
        rows.retain(|row| seen.insert(row.iter().map(|(k, v)| (k.clone(), v.clone())).collect()));
    }

    let rows = rows
        .into_iter()
        .skip(sel.offset)
        .take(sel.limit.unwrap_or(usize::MAX))
        .collect();

    (sel.project.clone(), rows)
}

/// Evaluate a plan against a specific graph `index` (the active graph).
/// `named_filter` (from `FROM NAMED`) restricts which graphs `GRAPH` may see.
pub(crate) fn eval_plan_in(
    rete: &Rete,
    index: &GraphIndex,
    named_filter: Option<&[String]>,
    plan: &Plan,
) -> Vec<Binding> {
    let recur = |p: &Plan| eval_plan_in(rete, index, named_filter, p);
    // A named graph is visible unless FROM NAMED excludes it.
    let visible = |name: &str| named_filter.is_none_or(|f| f.iter().any(|g| g == name));
    match plan {
        Plan::Bgp(patterns) => {
            let dict = rete.dictionary();
            eval_bgp_int_in(rete, index, patterns)
                .into_iter()
                .map(|ib| {
                    ib.into_iter()
                        .filter_map(|(k, v)| term_of_value(dict, v).map(|t| (k, t)))
                        .collect()
                })
                .collect()
        }
        Plan::Path(subj, spec, obj) => eval_path(rete, index, subj, spec, obj),
        Plan::Values(vars, rows) => rows
            .iter()
            .map(|row| {
                vars.iter()
                    .zip(row.iter())
                    .filter_map(|(v, val)| val.as_ref().map(|t| (v.clone(), t.clone())))
                    .collect()
            })
            .collect(),
        Plan::Filter(expr, inner) => {
            let mut v = recur(inner);
            let mut cache = ExistsCache::new();
            v.retain(|b| expr.boolean(rete, index, b, &mut cache));
            v
        }
        Plan::Union(l, r) => {
            let mut v = recur(l);
            v.extend(recur(r));
            v
        }
        Plan::Minus(l, r) => minus_hash(recur(l), recur(r)),
        Plan::Join(l, r) => values_pushdown(rete, index, l, r)
            .unwrap_or_else(|| hash_join_solutions(rete, index, recur(l), recur(r), false, None)),
        Plan::LeftJoin(l, r, cond) => {
            hash_join_solutions(rete, index, recur(l), recur(r), true, cond.as_ref())
        }
        // GRAPH switches the active graph index (subject to FROM NAMED).
        Plan::Graph(GraphTarget::Named(iri), inner) => match rete.graph_index(iri) {
            Some(gi) if visible(iri) => eval_plan_in(rete, gi, named_filter, inner),
            _ => Vec::new(),
        },
        Plan::Graph(GraphTarget::Var(var), inner) => {
            let mut out = Vec::new();
            for (name, gi) in rete.named_graphs() {
                if !visible(name) {
                    continue;
                }
                for mut sol in eval_plan_in(rete, gi, named_filter, inner) {
                    match sol.get(var) {
                        Some(existing) if existing != name => continue,
                        _ => {
                            sol.insert(var.clone(), name.clone());
                        }
                    }
                    out.push(sol);
                }
            }
            out
        }
    }
}

/// Merge two bindings if compatible (shared variables agree), else `None`.
fn merge(a: &Binding, b: &Binding) -> Option<Binding> {
    let mut out = a.clone();
    for (k, v) in b {
        match out.get(k) {
            Some(existing) if existing != v => return None,
            Some(_) => {}
            None => {
                out.insert(k.clone(), v.clone());
            }
        }
    }
    Some(out)
}

/// Substitute a binding's variables into a BGP's patterns, turning each bound
/// variable into a constant term so the index scan can constrain on it.
fn substitute_patterns(patterns: &[TriplePattern], input: &Binding) -> Vec<TriplePattern> {
    let sub = |t: &PatternTerm| -> PatternTerm {
        match t {
            PatternTerm::Var(v) => match input.get(v) {
                Some(val) => PatternTerm::Const(val.clone()),
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
fn values_pushdown(rete: &Rete, index: &GraphIndex, l: &Plan, r: &Plan) -> Option<Vec<Binding>> {
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
    let dict = rete.dictionary();
    let mut out = Vec::new();
    for row in rows {
        // The bound variables from this VALUES row (UNDEF entries stay variable).
        let input: Binding = vars
            .iter()
            .zip(row.iter())
            .filter_map(|(v, val)| val.as_ref().map(|t| (v.clone(), t.clone())))
            .collect();
        let subst = substitute_patterns(patterns, &input);
        for ib in eval_bgp_int_in(rete, index, &subst) {
            // Re-attach this row's VALUES bindings (the substituted vars no longer
            // appear in the BGP result), then the BGP's own bindings.
            let mut b = input.clone();
            for (k, v) in ib {
                if let Some(t) = term_of_value(dict, v) {
                    b.insert(k, t);
                }
            }
            out.push(b);
        }
    }
    Some(out)
}

/// Is a left row eliminated by a right row under `MINUS`? True iff they share at
/// least one variable and agree on every shared variable (SPARQL `MINUS`:
/// disjoint-domain rows never eliminate, and a disagreement keeps the left row).
fn minus_compatible(lb: &Binding, rb: &Binding) -> bool {
    let mut shared = false;
    for (k, v) in lb {
        if let Some(w) = rb.get(k) {
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
/// common case (both sides bind the shared variables in every row): the shared
/// variables `jv` index the right rows, so a fully-bound left row is eliminated
/// by a single set lookup. Rows missing a shared variable (only via a nested
/// OPTIONAL/UNION) fall back to a scan, preserving exact semantics.
fn minus_hash(left: Vec<Binding>, right: Vec<Binding>) -> Vec<Binding> {
    use std::collections::HashSet;
    if left.is_empty() || right.is_empty() {
        return left;
    }
    // Shared variables: those appearing in some left row AND some right row.
    let lvars: HashSet<&str> = left
        .iter()
        .flat_map(|b| b.keys().map(String::as_str))
        .collect();
    let mut jv: Vec<String> = right
        .iter()
        .flat_map(|b| b.keys().map(String::as_str))
        .filter(|v| lvars.contains(v))
        .map(String::from)
        .collect();
    jv.sort();
    jv.dedup();
    if jv.is_empty() {
        // Disjoint domains ⇒ MINUS eliminates nothing.
        return left;
    }
    let key_of =
        |b: &Binding| -> Option<Vec<String>> { jv.iter().map(|v| b.get(v).cloned()).collect() };
    // Right rows fully bound on the shared vars index a set; the rest must be
    // scanned (a left row could share only a sub-domain with them).
    let mut full: HashSet<Vec<String>> = HashSet::new();
    let mut partial_right: Vec<&Binding> = Vec::new();
    for r in &right {
        match key_of(r) {
            Some(k) => {
                full.insert(k);
            }
            None => partial_right.push(r),
        }
    }
    left.into_iter()
        .filter(|lb| {
            match key_of(lb) {
                // Fully bound on the shared vars: a fully-bound right row
                // eliminates it iff their keys match; otherwise only a partial
                // right row could.
                Some(k) => {
                    if full.contains(&k) {
                        return false;
                    }
                    !partial_right.iter().any(|rb| minus_compatible(lb, rb))
                }
                // Missing a shared var: must check every right row.
                None => !right.iter().any(|rb| minus_compatible(lb, rb)),
            }
        })
        .collect()
}

/// Hash join two solution sets on the variables they share, emitting every
/// compatible merge. `optional = true` is a left join (OPTIONAL): a left row
/// with no surviving match is emitted unchanged, and `cond` (the OPTIONAL's
/// filter) decides which merges count as a match.
///
/// This replaces the O(L×R) nested-loop merge with an O(L + R + matches) hash
/// join in the common case where the join variables are bound in every row.
/// Rows missing a join variable (only possible via a nested OPTIONAL) fall back
/// to being tried against all candidates, preserving exact SPARQL semantics —
/// `merge` still does the final compatibility check on every shared variable.
fn hash_join_solutions(
    rete: &Rete,
    index: &GraphIndex,
    left: Vec<Binding>,
    right: Vec<Binding>,
    optional: bool,
    cond: Option<&FExpr>,
) -> Vec<Binding> {
    use std::collections::{HashMap, HashSet};
    if left.is_empty() {
        return Vec::new();
    }
    if right.is_empty() {
        return if optional { left } else { Vec::new() };
    }
    // Join variables: names that occur in both sides.
    let lvars: HashSet<&str> = left
        .iter()
        .flat_map(|b| b.keys().map(String::as_str))
        .collect();
    let mut jset: HashSet<&str> = right
        .iter()
        .flat_map(|b| b.keys().map(String::as_str))
        .collect();
    jset.retain(|v| lvars.contains(v));
    let mut jv: Vec<String> = jset.into_iter().map(String::from).collect();
    jv.sort();

    // Key = the join-var values, when all are bound; `None` ⇒ a join var is
    // unbound in this row (a partial that must be matched against everything).
    let key_of =
        |b: &Binding| -> Option<Vec<String>> { jv.iter().map(|v| b.get(v).cloned()).collect() };
    let mut buckets: HashMap<Vec<String>, Vec<usize>> = HashMap::new();
    let mut partial: Vec<usize> = Vec::new();
    for (i, r) in right.iter().enumerate() {
        match key_of(r) {
            Some(k) => buckets.entry(k).or_default().push(i),
            None => partial.push(i),
        }
    }

    let mut out = Vec::new();
    let mut cache = ExistsCache::new();
    let mut emit = |a: &Binding, r: &Binding, out: &mut Vec<Binding>, matched: &mut bool| {
        if let Some(m) = merge(a, r) {
            if cond.is_none_or(|f| f.boolean(rete, index, &m, &mut cache)) {
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
            // `a` itself lacks a join var: every right row is a candidate.
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
