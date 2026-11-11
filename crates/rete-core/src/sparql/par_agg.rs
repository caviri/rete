//! A grouped query on several threads.
//!
//! `SELECT … (COUNT(*) AS ?n) WHERE { … } GROUP BY …` over ten million rows
//! spends its time per row: decoding a term, a string function, a lookup in
//! the row's graph. None of it depends on any other row, and the evaluator ran
//! all of it on one thread.
//!
//! Here the scan that drives the pattern is cut into [`SHARDS`] slices of its
//! tiles. Each worker thread takes slices in order, evaluates the *same* plan
//! over one slice at a time in its own context (its own term memo; the
//! contexts hold `Rc`s and never cross threads), and folds the rows into
//! partial aggregates. The slices' groups are merged in slice order, which is
//! scan order, so the answer is the one fold's answer: see the partial
//! aggregation notes in `aggregate`.
//!
//! **Which queries.** The driving scan has to sit where the algebra
//! distributes over a union of its rows, and nothing the plan evaluates beside
//! it may be repeated per slice at a cost. So the path from the top of the
//! pattern to the scan goes through FILTER and BIND (without EXISTS), UNION
//! (each branch split, or run in slice 0 alone when it cannot be), and the
//! left side of a join or OPTIONAL whose right side is `GRAPH ?g { … }` with
//! `?g` bound on the left (one lookup per row, nothing built). The scan
//! itself is one triple pattern: in a graph, or under `GRAPH ?g` on a
//! quad-index file. A hash join is not on the list: every slice would build
//! its right side again.
//!
//! **Checked, not assumed.** The plan nodes are named by address and every one
//! of them must be reached exactly once per slice ([`crate::row::Shard`]). A
//! pushdown that evaluates a node some other way would hand its rows to every
//! slice; the count is taken on a dry run before any thread starts and again
//! on every slice, and a mismatch means the query runs on one thread as
//! before.
//!
//! **Memory.** A slice holding more than [`MAX_SHARD_GROUPS`] groups stops the
//! attempt (the one fold then runs): a slice's groups wait for the slices
//! before it, and a query with millions of groups would hold them several
//! times over. Workers run at most `2 × workers` slices ahead of the merge.
//! Each worker's term memos are capped at its share of one context's.
//!
//! Native builds with the `parallel` feature only. `RETE_QUERY_THREADS=1` (or
//! [`set_query_threads`]) turns it off.

use std::sync::atomic::{AtomicUsize, Ordering};

use super::{GroupSpec, Select};
use crate::index::GraphIndex;
use crate::row::{Ctx, Row};

/// How many worker threads a grouped query may use; 0 = not set.
static THREADS: AtomicUsize = AtomicUsize::new(0);

/// The most threads used when nothing says otherwise.
const DEFAULT_MAX_THREADS: usize = 16;

/// Set how many threads a grouped query may run on (`1`: always one thread;
/// `0`: the default, the machine's cores up to 16, or `RETE_QUERY_THREADS`).
/// Process-wide. The rows a query returns do not depend on it.
pub fn set_query_threads(n: usize) {
    THREADS.store(n, Ordering::Relaxed);
}

/// How many threads a grouped query may run on: see [`set_query_threads`].
pub fn query_threads() -> usize {
    match THREADS.load(Ordering::Relaxed) {
        0 => default_threads(),
        n => n,
    }
}

fn default_threads() -> usize {
    static DEFAULT: std::sync::OnceLock<usize> = std::sync::OnceLock::new();
    *DEFAULT.get_or_init(|| {
        if let Some(n) = std::env::var("RETE_QUERY_THREADS")
            .ok()
            .and_then(|v| v.trim().parse::<usize>().ok())
            .filter(|&n| n > 0)
        {
            return n;
        }
        std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(1)
            .min(DEFAULT_MAX_THREADS)
    })
}

/// The group rows of `sel` (as `aggregate` returns them), computed on several
/// threads; `None` when the query is not one this can split, or the attempt
/// was given up: the caller then aggregates as it always did.
#[cfg(not(feature = "parallel"))]
pub(super) fn aggregate_shards<'q>(
    _ctx: &'q Ctx<'q>,
    _sel: &'q Select,
    _active: &'q GraphIndex,
    _nf: Option<&'q [String]>,
    _g: &'q GroupSpec,
) -> Option<Vec<Row>> {
    None
}

#[cfg(feature = "parallel")]
pub(super) use threaded::aggregate_shards;

#[cfg(feature = "parallel")]
mod threaded {
    use std::cell::Cell;
    use std::collections::BTreeMap;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{mpsc, Condvar, Mutex};

    use super::super::aggregate::{aggregate_partial, mergeable, Merger, PartialGroups};
    use super::super::eval::{certain_bound, eval_plan_iter, expr_has_exists, query_ctx};
    use super::super::{diag, GraphTarget, GroupSpec, Plan, Select};
    use super::query_threads;
    use crate::index::GraphIndex;
    use crate::row::{Ctx, Row, MEMO_CAP_BYTES};

    /// How many slices the driving scan is cut into (fewer when it has fewer
    /// tiles). Fixed, so the slices, and with them the order in which a
    /// floating-point SUM adds up, do not depend on the machine's core count.
    const SHARDS: usize = 64;

    /// The most groups one slice may hold: see the module notes on memory.
    const MAX_SHARD_GROUPS: usize = 100_000;

    /// The floor of a worker's share of the term memos.
    const MIN_MEMO_CAP: usize = 4 << 20;

    thread_local! {
        /// Set on a worker thread: a subquery it evaluates must not fan out again.
        static IN_WORKER: Cell<bool> = const { Cell::new(false) };
    }

    /// The plan nodes a grouped query is split on, by address.
    #[derive(Default)]
    struct Split {
        leaves: Vec<usize>,
        only_first: Vec<usize>,
    }

    /// Find the driving scans under `plan` (see the module notes). `false`:
    /// this pattern cannot be split, and `out` holds nothing to rely on.
    fn split(ctx: &Ctx, plan: &Plan, from_named: bool, out: &mut Split) -> bool {
        let node = plan as *const Plan as usize;
        match plan {
            Plan::Bgp(patterns) if patterns.len() == 1 => {
                out.leaves.push(node);
                true
            }
            Plan::Graph(GraphTarget::Var(v), inner) => {
                let one = matches!(&**inner, Plan::Bgp(p) if p.len() == 1);
                if one
                    && !from_named
                    && ctx.rete.quad_named().is_some()
                    && ctx.slots.slot(v).is_some()
                {
                    out.leaves.push(node);
                    true
                } else {
                    false
                }
            }
            Plan::Graph(GraphTarget::Named(_), inner) => split(ctx, inner, from_named, out),
            Plan::Filter(e, inner) | Plan::Extend(_, e, inner) => {
                !expr_has_exists(e) && split(ctx, inner, from_named, out)
            }
            Plan::Union(l, r) => {
                // A branch that cannot be split runs whole, in slice 0 alone.
                let mut branch = |p: &Plan| {
                    let (a, b) = (out.leaves.len(), out.only_first.len());
                    if split(ctx, p, from_named, out) {
                        true
                    } else {
                        out.leaves.truncate(a);
                        out.only_first.truncate(b);
                        out.only_first.push(p as *const Plan as usize);
                        false
                    }
                };
                let l = branch(l);
                let r = branch(r);
                l || r
            }
            Plan::Join(l, r) => looked_up_per_row(ctx, l, r) && split(ctx, l, from_named, out),
            Plan::LeftJoin(l, r, cond) => {
                cond.as_ref().is_none_or(|c| !expr_has_exists(c))
                    && looked_up_per_row(ctx, l, r)
                    && split(ctx, l, from_named, out)
            }
            _ => false,
        }
    }

    /// Is `r` joined to `l` by one lookup per left row, with nothing built
    /// from `r` as a whole? `GRAPH ?g { … }` with `?g` certainly bound on the
    /// left (`graph_bound_join`).
    fn looked_up_per_row(ctx: &Ctx, l: &Plan, r: &Plan) -> bool {
        let Plan::Graph(GraphTarget::Var(gv), _) = r else {
            return false;
        };
        let Some(slot) = ctx.slots.slot(gv) else {
            return false;
        };
        certain_bound(ctx, l, ctx.slots.len())[slot]
    }

    /// Which slice a worker takes next, and how far the merge has come.
    struct Claims {
        next: usize,
        merged: usize,
        stop: bool,
    }

    enum Msg {
        /// Slice `i`'s groups; `None`: the attempt is given up.
        Part(usize, Option<PartialGroups>),
        /// The type errors a worker's expressions recorded.
        Diag(diag::Taken),
    }

    pub(in super::super) fn aggregate_shards<'q>(
        ctx: &'q Ctx<'q>,
        sel: &'q Select,
        active: &'q GraphIndex,
        nf: Option<&'q [String]>,
        g: &'q GroupSpec,
    ) -> Option<Vec<Row>> {
        let threads = query_threads();
        if threads < 2 || IN_WORKER.with(Cell::get) || crate::guard::armed_here() {
            return None;
        }
        if !mergeable(g) || g.pre.iter().any(|(_, e)| expr_has_exists(e)) {
            return None;
        }
        let mut sp = Split::default();
        if !split(ctx, &sel.plan, nf.is_some(), &mut sp) || sp.leaves.is_empty() {
            return None;
        }
        let expected = (sp.leaves.len() + sp.only_first.len()) as u32;

        // Dry run: build slice 0's pipeline in this context, pull nothing. It
        // must reach every split node once, and it tells how many tiles the
        // longest driving scan has.
        ctx.shard.set_plan(sp.leaves.clone(), sp.only_first.clone());
        ctx.shard.begin(Some((0, SHARDS)));
        drop(eval_plan_iter(ctx, active, nf, &sel.plan));
        let (hits, tiles) = (ctx.shard.hits(), ctx.shard.tiles());
        ctx.shard.end_worker();
        let n = SHARDS.min(tiles);
        if hits != expected || n < 2 {
            return None;
        }
        let workers = threads.min(n);
        let window = 2 * workers;
        let memo_cap = (MEMO_CAP_BYTES / workers).max(MIN_MEMO_CAP);

        let claims = Mutex::new(Claims {
            next: 0,
            merged: 0,
            stop: false,
        });
        let wake = Condvar::new();
        let abort = AtomicBool::new(false);
        let give_up = || {
            abort.store(true, Ordering::Relaxed);
            claims.lock().unwrap().stop = true;
            wake.notify_all();
        };
        let rete = ctx.rete;
        let (tx, rx) = mpsc::channel::<Msg>();
        let mut merger = Merger::default();
        let mut failed = false;

        std::thread::scope(|scope| {
            for _ in 0..workers {
                let tx = tx.clone();
                let (claims, wake, abort, give_up, sp) = (&claims, &wake, &abort, &give_up, &sp);
                scope.spawn(move || {
                    IN_WORKER.with(|w| w.set(true));
                    diag::begin();
                    let mut wctx = query_ctx(rete, sel);
                    wctx.resolver.set_memo_cap(memo_cap);
                    wctx.exhaustive.set(true);
                    wctx.shard
                        .set_plan(sp.leaves.clone(), sp.only_first.clone());
                    let wctx = &wctx;
                    loop {
                        let i = {
                            let mut c = claims.lock().unwrap();
                            loop {
                                if c.stop || c.next >= n {
                                    break None;
                                }
                                if c.next < c.merged + window {
                                    c.next += 1;
                                    break Some(c.next - 1);
                                }
                                c = wake.wait(c).unwrap();
                            }
                        };
                        let Some(i) = i else { break };
                        wctx.shard.begin(Some((i, n)));
                        let mut seen = 0u32;
                        let rows = eval_plan_iter(wctx, active, nf, &sel.plan).take_while(|_| {
                            seen = seen.wrapping_add(1);
                            seen % 4096 != 0 || !abort.load(Ordering::Relaxed)
                        });
                        let part = aggregate_partial(wctx, rows, g, MAX_SHARD_GROUPS)
                            .filter(|_| {
                                wctx.shard.hits() == expected && !abort.load(Ordering::Relaxed)
                            });
                        let gone = part.is_none();
                        if gone {
                            give_up();
                        }
                        if tx.send(Msg::Part(i, part)).is_err() || gone {
                            break;
                        }
                    }
                    let _ = tx.send(Msg::Diag(diag::take()));
                });
            }
            drop(tx);

            // Merge in slice order as the slices arrive.
            let mut pending: BTreeMap<usize, PartialGroups> = BTreeMap::new();
            let mut next = 0usize;
            for msg in rx {
                match msg {
                    Msg::Diag(taken) => diag::absorb(taken),
                    Msg::Part(_, None) => {
                        failed = true;
                        pending.clear();
                        give_up();
                    }
                    Msg::Part(i, Some(part)) => {
                        if failed {
                            continue;
                        }
                        pending.insert(i, part);
                        while let Some(part) = pending.remove(&next) {
                            merger.absorb(part);
                            next += 1;
                        }
                        claims.lock().unwrap().merged = next;
                        wake.notify_all();
                    }
                }
            }
            if next != n {
                failed = true;
            }
        });

        if failed {
            return None;
        }
        Some(merger.finish(ctx, g))
    }
}
