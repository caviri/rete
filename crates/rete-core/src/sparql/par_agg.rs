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
//! contexts hold `Arc`s and never cross threads), and folds the rows into
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
//! slice; this is checked on a dry run before any thread starts and again on
//! every slice, and a miss means the query runs on one thread as before.
//!
//! **Memory.** A slice holding more than [`MAX_SHARD_GROUPS`] groups stops the
//! attempt (the one fold then runs): a slice's groups wait for the slices
//! before it, and a query with millions of groups would hold them several
//! times over. Workers run at most `2 × workers` slices ahead of the merge.
//! Each worker's term memos are capped at its share of one context's.
//!
//! **Local reads only.** A scan over a remote file reads ahead in a few
//! growing batches; sixty-four slices would each start small, and the same
//! bytes would come in many more requests. So a file whose reads are round
//! trips ([`crate::Rete::reads_are_remote`]) keeps its one scan.
//!
//! Native builds with the `parallel` feature only. `RETE_QUERY_THREADS=1` (or
//! [`set_query_threads`]) turns it off.

use std::sync::atomic::{AtomicUsize, Ordering};

/// How many worker threads a grouped query may use; 0 = not set.
static THREADS: AtomicUsize = AtomicUsize::new(0);

/// Grouped queries answered on several threads so far, process-wide.
static SHARDED: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// How many grouped queries (subqueries included) have been answered on
/// several threads so far, in this process. A counter for tests and
/// diagnostics: whether a query took that path is the difference around it.
pub fn sharded_queries() -> u64 {
    SHARDED.load(Ordering::Relaxed)
}

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
    _ctx: &'q crate::row::Ctx<'q>,
    _sel: &'q super::Select,
    _active: &'q crate::index::GraphIndex,
    _nf: Option<&'q [String]>,
    _g: &'q super::GroupSpec,
) -> Option<Vec<crate::row::Row>> {
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

    use super::super::aggregate::{
        aggregate_folded, aggregate_partial, apply_pre, fold_slots, mergeable, Merger,
        PartialGroups,
    };
    use super::super::eval::{
        certain_bound, eval_plan_iter, expr_has_exists, path_endpoint_bound, query_ctx,
        shares_certain_var,
    };
    use super::super::{diag, GraphTarget, GroupSpec, Plan, Select};
    use super::query_threads;
    use crate::index::GraphIndex;
    use crate::row::{Ctx, Row, SharedState, MEMO_CAP_BYTES};

    /// How many slices the driving scan is cut into (fewer when it has fewer
    /// tiles). Fixed, so the slices, and with them the order in which a
    /// floating-point SUM adds up, do not depend on the machine's core count.
    const SHARDS: usize = 64;

    /// The most groups one slice may hold when the slices' groups are merged:
    /// see the module notes on memory.
    const MAX_SHARD_GROUPS: usize = 100_000;

    /// Rows per message when the workers send rows to the one fold.
    const ROW_CHUNK: usize = 2048;

    /// About how many rows may wait for the one fold, over all workers.
    const ROWS_AHEAD: usize = 1 << 21;

    /// How many slices the driving scan is cut into when the rows go to one
    /// fold. The fold sees the scan's rows in the scan's order whatever the
    /// slices are, so they can be small: a worker then finishes a slice
    /// before its share of the waiting rows is full, and does not stop to
    /// wait for the fold.
    const ROW_SHARDS: usize = 1024;

    /// A worker's stack: the main thread's usual 8 MiB, not a spawned
    /// thread's 2 MiB, since a worker evaluates the same (possibly deeply
    /// nested) plan the calling thread would.
    const WORKER_STACK: usize = 8 << 20;

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
        /// Joins whose right side is built once for all slices.
        builds: Vec<usize>,
    }

    /// Find the driving scans under `plan` (see the module notes); `index` is
    /// the graph `plan` is evaluated on. `false`: this pattern cannot be
    /// split, and `out` holds nothing to rely on.
    fn split(
        ctx: &Ctx,
        plan: &Plan,
        index: &GraphIndex,
        from_named: bool,
        out: &mut Split,
    ) -> bool {
        let node = plan as *const Plan as usize;
        match plan {
            Plan::Bgp(patterns) if !patterns.is_empty() => {
                out.leaves.push(node);
                true
            }
            // Any block under a graph variable: its one-pattern scan, the
            // graphs its required pattern is found in, or every graph, sliced.
            Plan::Graph(GraphTarget::Var(v), _) => {
                if ctx.slots.slot(v).is_some() {
                    out.leaves.push(node);
                    true
                } else {
                    false
                }
            }
            Plan::Graph(GraphTarget::Named(iri), inner) => match ctx.rete.graph_index(iri) {
                Some(gi) => split(ctx, inner, gi, from_named, out),
                None => false,
            },
            Plan::Filter(e, inner) | Plan::Extend(_, e, inner) => {
                !expr_has_exists(e) && split(ctx, inner, index, from_named, out)
            }
            Plan::Union(l, r) => {
                // A branch that cannot be split runs whole, in slice 0 alone.
                let mut branch = |p: &Plan| {
                    let mark = (out.leaves.len(), out.only_first.len(), out.builds.len());
                    if split(ctx, p, index, from_named, out) {
                        true
                    } else {
                        out.leaves.truncate(mark.0);
                        out.only_first.truncate(mark.1);
                        out.builds.truncate(mark.2);
                        out.only_first.push(p as *const Plan as usize);
                        false
                    }
                };
                let l = branch(l);
                let r = branch(r);
                l || r
            }
            Plan::Join(l, r) => join(ctx, node, l, r, index, from_named, out),
            Plan::LeftJoin(l, r, cond) => {
                cond.as_ref().is_none_or(|c| !expr_has_exists(c))
                    && join(ctx, node, l, r, index, from_named, out)
            }
            _ => false,
        }
    }

    /// `l ⋈ r` (or `l OPTIONAL r`) with the driving scan under `l`. The right
    /// side is either looked up once per left row, as the single thread would
    /// (nothing is built), or built once into a hash table every slice probes.
    fn join(
        ctx: &Ctx,
        node: usize,
        l: &Plan,
        r: &Plan,
        index: &GraphIndex,
        from_named: bool,
        out: &mut Split,
    ) -> bool {
        if looked_up_per_row(ctx, index, l, r) {
            return split(ctx, l, index, from_named, out);
        }
        if !buildable(r) {
            return false;
        }
        let mark = out.builds.len();
        out.builds.push(node);
        if split(ctx, l, index, from_named, out) {
            true
        } else {
            out.builds.truncate(mark);
            false
        }
    }

    /// Is `r` joined to `l` by one lookup per left row, whatever the rows,
    /// with nothing built from `r` as a whole? The three cases `join_iter`
    /// decides before it sees a row: `GRAPH ?g { … }` with `?g` certainly
    /// bound on the left, a property path with an endpoint bound on the left,
    /// and a BGP that shares a certainly bound variable and whose cheapest
    /// pattern is a fat scan.
    fn looked_up_per_row(ctx: &Ctx, index: &GraphIndex, l: &Plan, r: &Plan) -> bool {
        let lcert = certain_bound(ctx, l, ctx.slots.len());
        match r {
            Plan::Graph(GraphTarget::Var(gv), _) => {
                ctx.slots.slot(gv).is_some_and(|slot| lcert[slot])
            }
            Plan::Path(s, _, o) => path_endpoint_bound(ctx, l, s, o),
            Plan::Bgp(patterns) => {
                !patterns.is_empty()
                    && shares_certain_var(ctx, patterns, &lcert)
                    && crate::bgp::bgp_min_estimate(ctx, index, patterns)
                        .is_none_or(|n| n >= crate::bgp::FAT_SCAN_ROWS)
            }
            _ => false,
        }
    }

    /// May the right side of a join be evaluated whole and built into a
    /// table? The shapes the single thread hash-joins. Not the ones it would
    /// rather push the left rows into (VALUES, a UNION of patterns, an
    /// entailed atom), nor a path, which it drives from a bound endpoint.
    fn buildable(r: &Plan) -> bool {
        match r {
            Plan::Bgp(_) | Plan::Subquery(_) => true,
            Plan::Graph(_, inner) => buildable(inner),
            Plan::Filter(e, inner) | Plan::Extend(_, e, inner) => {
                !expr_has_exists(e) && buildable(inner)
            }
            Plan::Join(a, b) => buildable(a) && buildable(b),
            Plan::LeftJoin(a, b, cond) => {
                cond.as_ref().is_none_or(|c| !expr_has_exists(c)) && buildable(a) && buildable(b)
            }
            _ => false,
        }
    }

    /// Runs its closure when dropped by a panic's unwinding.
    struct OnPanic<F: Fn()>(F);

    impl<F: Fn()> Drop for OnPanic<F> {
        fn drop(&mut self) {
            if std::thread::panicking() {
                (self.0)();
            }
        }
    }

    /// Which slice a worker takes next, and how far the calling thread has
    /// come (slices merged, or the slice whose rows it is folding).
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

    /// How a run on several threads ended.
    enum Outcome {
        Done(Vec<Row>),
        /// A slice held more groups than may wait for a merge.
        TooManyGroups,
        Failed,
    }

    /// What every run needs: the query, the split, and what was built for all
    /// slices.
    struct Job<'j, 'q> {
        rete: &'q crate::file::Rete,
        sel: &'q Select,
        active: &'q GraphIndex,
        nf: Option<&'q [String]>,
        g: &'q GroupSpec,
        sp: &'j Split,
        shared: &'j SharedState,
        /// The number of slices when their groups are merged.
        n: usize,
        /// The length of the longest driving scan, in tiles (or rows).
        tiles: usize,
        workers: usize,
    }

    impl Job<'_, '_> {
        /// A worker's own context: its share of the memos, the split, and
        /// what is shared.
        fn worker_ctx(&self) -> Ctx<'_> {
            let mut wctx = query_ctx(self.rete, self.sel);
            let memo_cap = (MEMO_CAP_BYTES / self.workers).max(MIN_MEMO_CAP);
            wctx.resolver.set_memo_cap(memo_cap);
            wctx.exhaustive.set(true);
            wctx.shard.set_plan(
                self.sp.leaves.clone(),
                self.sp.only_first.clone(),
                self.sp.builds.clone(),
                self.shared.clone(),
            );
            wctx
        }
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
        // Over a network one scan batches its reads into a few requests; cut
        // into slices it would make many small ones. Local reads only.
        if ctx.rete.reads_are_remote() {
            return None;
        }
        if g.pre.iter().any(|(_, e)| expr_has_exists(e)) {
            return None;
        }
        let mut sp = Split::default();
        if !split(ctx, &sel.plan, active, nf.is_some(), &mut sp) || sp.leaves.is_empty() {
            return None;
        }

        // Dry run: build slice 0's pipeline in this context, pull nothing. It
        // must reach every split node once; it builds what the slices share
        // (a join's table, a pattern's prefix); and it tells how long the
        // longest driving scan is.
        ctx.shard.set_plan(
            sp.leaves.clone(),
            sp.only_first.clone(),
            sp.builds.clone(),
            SharedState::new(),
        );
        ctx.shard.begin(Some((0, SHARDS)));
        let t0 = std::time::Instant::now();
        drop(eval_plan_iter(ctx, active, nf, &sel.plan));
        let (exact, tiles) = (ctx.shard.exact(), ctx.shard.tiles());
        let shared = ctx.shard.shared_state();
        ctx.shard.clear();
        let n = SHARDS.min(tiles);
        if std::env::var_os("RETE_DEBUG_SHARDS").is_some() {
            eprintln!(
                "shards: exact={exact} tiles={tiles} leaves={} builds={} dry={:?}",
                sp.leaves.len(),
                sp.builds.len(),
                t0.elapsed()
            );
        }
        if !exact || n < 2 {
            return None;
        }
        let job = Job {
            rete: ctx.rete,
            sel,
            active,
            nf,
            g,
            sp: &sp,
            shared: &shared,
            n,
            tiles,
            workers: threads.min(n),
        };
        // The slices' groups are merged when every aggregate can be; when one
        // cannot (a set of distinct values), or a slice has too many groups
        // to hold while it waits, the workers send their rows to one fold.
        let outcome = if mergeable(g) {
            match merge_groups(ctx, &job) {
                Outcome::TooManyGroups => fold_rows(ctx, &job),
                other => other,
            }
        } else {
            fold_rows(ctx, &job)
        };
        if std::env::var_os("RETE_DEBUG_SHARDS").is_some() {
            eprintln!("shards: done after {:?}", t0.elapsed());
        }
        match outcome {
            Outcome::Done(rows) => {
                super::SHARDED.fetch_add(1, Ordering::Relaxed);
                Some(rows)
            }
            _ => None,
        }
    }

    /// Each worker folds its slices into partial aggregates; the calling
    /// thread merges them in slice order.
    fn merge_groups(ctx: &Ctx, job: &Job) -> Outcome {
        let (n, workers) = (job.n, job.workers);
        let window = 2 * workers;
        let claims = Mutex::new(Claims {
            next: 0,
            merged: 0,
            stop: false,
        });
        let wake = Condvar::new();
        let abort = AtomicBool::new(false);
        let too_many = AtomicBool::new(false);
        let give_up = || {
            abort.store(true, Ordering::Relaxed);
            if let Ok(mut c) = claims.lock() {
                c.stop = true;
            }
            wake.notify_all();
        };
        let (tx, rx) = mpsc::channel::<Msg>();
        let mut merger = Merger::default();
        let mut failed = false;

        std::thread::scope(|scope| {
            for _ in 0..workers {
                let tx = tx.clone();
                let (claims, wake, abort, too_many, give_up) =
                    (&claims, &wake, &abort, &too_many, &give_up);
                let worker = move || {
                    // A worker that panics never sends its slice: the others
                    // must not wait for the merge to pass it.
                    let _stop = OnPanic(give_up);
                    IN_WORKER.with(|w| w.set(true));
                    diag::begin();
                    let wctx = job.worker_ctx();
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
                        let rows = eval_plan_iter(wctx, job.active, job.nf, &job.sel.plan)
                            .take_while(|_| {
                                seen = seen.wrapping_add(1);
                                !seen.is_multiple_of(4096) || !abort.load(Ordering::Relaxed)
                            });
                        let folded = aggregate_partial(wctx, rows, job.g, MAX_SHARD_GROUPS);
                        if folded.is_none() {
                            too_many.store(true, Ordering::Relaxed);
                        }
                        let part =
                            folded.filter(|_| wctx.shard.exact() && !abort.load(Ordering::Relaxed));
                        let gone = part.is_none();
                        if gone {
                            give_up();
                        }
                        if tx.send(Msg::Part(i, part)).is_err() || gone {
                            break;
                        }
                    }
                    let _ = tx.send(Msg::Diag(diag::take()));
                };
                // A thread that cannot be started is one worker fewer; with
                // none at all no slice arrives and the attempt fails.
                if std::thread::Builder::new()
                    .name("rete-query".into())
                    .stack_size(WORKER_STACK)
                    .spawn_scoped(scope, worker)
                    .is_err()
                {
                    break;
                }
            }
            drop(tx);

            // Merge in slice order as the slices arrive.
            let mut pending: BTreeMap<usize, PartialGroups> = BTreeMap::new();
            let mut next = 0usize;
            let mut diags = Vec::new();
            for msg in rx {
                match msg {
                    Msg::Diag(taken) => diags.push(taken),
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
            // Only a run that is kept reports what its workers recorded: a
            // run given up is evaluated again, and records it all again.
            if !failed {
                diags.into_iter().for_each(diag::absorb);
            }
        });

        if failed {
            return if too_many.load(Ordering::Relaxed) {
                Outcome::TooManyGroups
            } else {
                Outcome::Failed
            };
        }
        Outcome::Done(merger.finish(ctx, job.g))
    }

    /// Each worker evaluates its slices and sends the rows; the calling
    /// thread folds them, slice after slice, exactly as it folds the one
    /// scan's rows. Whatever the aggregates keep (distinct sets, any number of
    /// groups) is kept once, by the one fold. The workers do the per-row work
    /// of the pattern and of the aggregated expressions.
    fn fold_rows(ctx: &Ctx, job: &Job) -> Outcome {
        let (n, workers) = (ROW_SHARDS.min(job.tiles), job.workers);
        // The slots the fold reads: a row is cut down to them before it is
        // sent, so what waits for the fold is a few values a row.
        let keep = fold_slots(ctx, job.g);
        let keep = &keep;
        // A slice's rows wait in its own bounded channel, so a worker ahead
        // of the fold stops when its share of the rows that may wait is full.
        let chunks_ahead = (ROWS_AHEAD / workers / ROW_CHUNK).max(1);
        let mut senders = Vec::with_capacity(n);
        let mut receivers = Vec::with_capacity(n);
        for _ in 0..n {
            let (tx, rx) = mpsc::sync_channel::<Vec<Row>>(chunks_ahead);
            senders.push(Some(tx));
            receivers.push(rx);
        }
        let senders = Mutex::new(senders);
        let claims = Mutex::new(Claims {
            next: 0,
            merged: 0,
            stop: false,
        });
        let wake = Condvar::new();
        let abort = AtomicBool::new(false);
        let give_up = || {
            abort.store(true, Ordering::Relaxed);
            if let Ok(mut c) = claims.lock() {
                c.stop = true;
            }
            // A slice nobody took has its sender here: dropped, so the fold
            // does not wait for it.
            if let Ok(mut s) = senders.lock() {
                s.iter_mut().for_each(|tx| *tx = None);
            }
            wake.notify_all();
        };
        let (dtx, drx) = mpsc::channel::<diag::Taken>();
        let mut out = Vec::new();

        std::thread::scope(|scope| {
            for _ in 0..workers {
                let dtx = dtx.clone();
                let (claims, wake, abort, give_up, senders) =
                    (&claims, &wake, &abort, &give_up, &senders);
                let worker = move || {
                    let _stop = OnPanic(give_up);
                    IN_WORKER.with(|w| w.set(true));
                    diag::begin();
                    let wctx = job.worker_ctx();
                    let wctx = &wctx;
                    loop {
                        // At most `2 × workers` slices ahead of the one being
                        // folded: what waits is bounded by slices in flight.
                        let i = {
                            let mut c = claims.lock().unwrap();
                            loop {
                                if c.stop || c.next >= n {
                                    break None;
                                }
                                if c.next < c.merged + 2 * workers {
                                    c.next += 1;
                                    break Some(c.next - 1);
                                }
                                c = wake.wait(c).unwrap();
                            }
                        };
                        let Some(i) = i else { break };
                        let Some(tx) = senders.lock().unwrap()[i].take() else {
                            break;
                        };
                        wctx.shard.begin(Some((i, n)));
                        let mut chunk: Vec<Row> = Vec::with_capacity(ROW_CHUNK);
                        let mut open = true;
                        for mut row in eval_plan_iter(wctx, job.active, job.nf, &job.sel.plan) {
                            apply_pre(wctx, &mut row, job.g);
                            chunk.push(keep.iter().map(|&s| row[s].take()).collect());
                            if chunk.len() == ROW_CHUNK {
                                let full =
                                    std::mem::replace(&mut chunk, Vec::with_capacity(ROW_CHUNK));
                                if tx.send(full).is_err() || abort.load(Ordering::Relaxed) {
                                    open = false;
                                    break;
                                }
                            }
                        }
                        if open && !chunk.is_empty() {
                            open = tx.send(chunk).is_ok();
                        }
                        if !wctx.shard.exact() {
                            give_up();
                        }
                        if !open || abort.load(Ordering::Relaxed) {
                            break;
                        }
                    }
                    let _ = dtx.send(diag::take());
                };
                if std::thread::Builder::new()
                    .name("rete-query".into())
                    .stack_size(WORKER_STACK)
                    .spawn_scoped(scope, worker)
                    .is_err()
                {
                    // Fewer workers; with none, no slice would ever be taken.
                    give_up();
                    break;
                }
            }
            drop(dtx);

            // The one fold: slice after slice, each slice's rows in order.
            let (claims, wake, abort) = (&claims, &wake, &abort);
            let rows = receivers
                .into_iter()
                .enumerate()
                .flat_map(move |(i, rx)| {
                    if let Ok(mut c) = claims.lock() {
                        c.merged = i;
                    }
                    wake.notify_all();
                    rx.into_iter().flatten()
                })
                .take_while(|_| !abort.load(Ordering::Relaxed));
            out = aggregate_folded(ctx, rows, job.g, keep);
            if abort.load(Ordering::Relaxed) {
                // Workers blocked on a full channel see it closed (the
                // receivers went with the iterator) and stop.
                give_up();
            } else {
                // Every slice was folded to its end; the workers are done or
                // about to be. Their records come once they have all gone.
                wake.notify_all();
            }
        });
        if abort.load(Ordering::Relaxed) {
            return Outcome::Failed;
        }
        drx.into_iter().for_each(diag::absorb);
        Outcome::Done(out)
    }
}
