//! `GRAPH ?g { ?s a <C> . ?s ?p ?o ... }` over a quad index by **subject probes
//! into SPOG** (`0x07`, SPEC.md section 6.6), instead of the graph-by-graph
//! walk.
//!
//! The walk finds the graphs that hold a `C` (POSG), then evaluates the block
//! in each graph over its GSPO tiles, whose cost is the tiles of every such
//! graph, however few of their rows are the `C`'s. SPOG sorts the rows of a
//! subject together, and a file written with `--cluster-subjects-by-class`
//! puts the subjects of one class into one contiguous run of tiles, so probing
//! SPOG for the `C`'s subjects reads that run and nothing else.
//!
//! The route:
//!
//! 1. the `(subject, graph)` pairs of `?s a <C>` from POSG, subjects ascending;
//! 2. choose between this route and the walk by the bytes each reads, estimated
//!    from the directory the root lists ([`QuadNamed::probe_vs_walk_bytes`]):
//!    the probe unless it would read half as much again as the walk;
//! 3. probe SPOG for the subjects in windows that grow fourfold from 256 to
//!    65,536 subjects ([`QuadNamed::subject_rows`]: one batch of leaves and one
//!    of tiles per window, the run of `C` first), the dictionary terms the
//!    final projection will decode fetched a window at a time;
//! 4. per subject and graph where it is a `C`, match every pattern of the block
//!    against that graph's rows of the subject.
//!
//! Only a star on one subject variable qualifies. Everything else keeps the walk.

use super::*;
use crate::file::RDF_TYPE;
use crate::quad::QuadNamed;
use crate::quad7::Row4;

/// Subjects in the first window; each next window has four times as many.
const FIRST_WINDOW: usize = 256;
/// Subjects in the biggest window.
const MAX_WINDOW: usize = 1 << 16;
/// A demand this small keeps the walk.
const SMALL_LIMIT: usize = 4096;
/// Most `(subject, graph)` pairs the route collects (8 bytes each); a bigger
/// class keeps the walk.
const MAX_PAIRS: usize = 64 << 20;

/// 0: choose by the estimate, 1: always probe, 2: always walk, 255: not read yet.
static ROUTE: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(255);

/// Force the route of a class-scoped `GRAPH ?g` query: `Some(true)` probes
/// SPOG whenever the shape allows it, `Some(false)` always walks the graphs,
/// `None` chooses by the estimate (the default; also the environment variable
/// `RETE_CLASS_ROUTE=probe|walk`, read once). For measurements and tests; every
/// route returns the same rows.
pub fn set_class_route(force: Option<bool>) {
    let v = match force {
        None => 0,
        Some(true) => 1,
        Some(false) => 2,
    };
    ROUTE.store(v, std::sync::atomic::Ordering::Relaxed);
}

/// Which route to take: the estimate (default), or forced.
fn forced() -> Option<bool> {
    use std::sync::atomic::Ordering::Relaxed;
    if ROUTE.load(Relaxed) == 255 {
        let v = match std::env::var("RETE_CLASS_ROUTE").as_deref() {
            Ok("probe") => 1,
            Ok("walk") => 2,
            _ => 0,
        };
        let _ = ROUTE.compare_exchange(255, v, Relaxed, Relaxed);
    }
    match ROUTE.load(Relaxed) {
        1 => Some(true),
        2 => Some(false),
        _ => None,
    }
}

/// The rows of `patterns` (a star on one subject) in the graphs where the
/// subject has the type of one of them, by probing SPOG. `None` when the shape
/// or the file does not allow it, or the walk is estimated to read less.
pub(crate) fn class_probe_rows<'q>(
    ctx: &'q Ctx<'q>,
    patterns: &[TriplePattern],
    gslot: usize,
) -> Option<Box<dyn Iterator<Item = Row> + 'q>> {
    let force = forced();
    if force == Some(false) || patterns.len() < 2 {
        return None;
    }
    // A small LIMIT stops the walk after its first graphs: nothing to gain, and the
    // probe would first place the class's subjects (measured: 1.8 s to 2.8 s).
    if force != Some(true) && ctx.limit_hint.get().is_some_and(|n| n <= SMALL_LIMIT) {
        return None;
    }
    let quad = ctx.rete.quad_named()?;
    if !quad.has_subject_probe() {
        return None;
    }
    let dict = ctx.rete.dictionary();
    let lowered = lower(patterns, ctx)?;
    // A star on one subject variable, the graph variable nowhere inside.
    let SlotTerm::Var(sslot) = lowered[0].0 else {
        return None;
    };
    if sslot == gslot {
        return None;
    }
    let mut consts: Vec<(Option<u32>, Option<u32>)> = Vec::with_capacity(lowered.len());
    for t in &lowered {
        if !matches!(t.0, SlotTerm::Var(s) if s == sslot) {
            return None;
        }
        if [&t.1, &t.2]
            .iter()
            .any(|x| matches!(x, SlotTerm::Var(v) if *v == gslot))
        {
            return None;
        }
        // An impossible constant (a node as predicate): leave it to the walk.
        let cp = const_predicate(&t.1)?;
        let co = const_object(&t.2, dict)?;
        consts.push((cp, co));
    }
    // The pattern that names the class: `?s rdf:type <C>`.
    let pid = dict.predicate_id(RDF_TYPE)?;
    let class = consts
        .iter()
        .find_map(|&(cp, co)| if cp == Some(pid) { co } else { None })?;
    // 1. The pairs.
    let scan = quad.scan_pairs((None, Some(pid), Some(class)))?;
    if scan.is_empty() {
        return Some(Box::new(std::iter::empty()));
    }
    if scan.len() > MAX_PAIRS {
        return None;
    }
    let (subjects, graphs) = scan.subjects_and_graphs();
    // 2. The route.
    if force != Some(true) {
        let (probe, walk) = quad.probe_vs_walk_bytes(&subjects, &graphs, Some(class))?;
        // The walk also pays a request or two per graph and resolves every graph
        // name, which the probe does not (measured on the nanopub dump: the same
        // bytes, 213 s against 103 s), so the probe is taken unless it is
        // estimated to read half as much again as the walk.
        if probe.saturating_mul(2) > walk.saturating_mul(3) {
            return None;
        }
    }
    Some(Box::new(ClassProbe {
        ctx,
        quad,
        lowered,
        consts,
        gslot,
        sslot,
        class,
        scan,
        subjects,
        cursor: 0,
        window: FIRST_WINDOW,
        out: std::collections::VecDeque::new(),
    }))
}

/// `(subject, graph)` pairs, ascending.
pub(crate) struct Pairs(Vec<(u32, u32)>);

impl Pairs {
    fn len(&self) -> usize {
        self.0.len()
    }
    fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
    /// The distinct subjects and the distinct graphs, each ascending.
    fn subjects_and_graphs(&self) -> (Vec<u32>, Vec<u32>) {
        let mut subjects: Vec<u32> = self.0.iter().map(|p| p.0).collect();
        subjects.dedup();
        let mut graphs: Vec<u32> = self.0.iter().map(|p| p.1).collect();
        graphs.sort_unstable();
        graphs.dedup();
        (subjects, graphs)
    }
    /// The graphs of subject `s`.
    fn graphs_of(&self, s: u32) -> &[(u32, u32)] {
        let lo = self.0.partition_point(|p| p.0 < s);
        let hi = lo + self.0[lo..].partition_point(|p| p.0 == s);
        &self.0[lo..hi]
    }
}

impl QuadNamed {
    /// The `(subject, graph)` pairs of the quads matching `pat` (ids), sorted
    /// and distinct. `None` when the file cannot answer, or a read failed.
    fn scan_pairs(&self, pat: (Option<u32>, Option<u32>, Option<u32>)) -> Option<Pairs> {
        let scan = self.scan(pat)?;
        let mut v: Vec<(u32, u32)> = Vec::new();
        for (g, (s, _, _)) in scan {
            v.push((s, g));
            if v.len() > MAX_PAIRS {
                break;
            }
        }
        v.sort_unstable();
        v.dedup();
        Some(Pairs(v))
    }
}

struct ClassProbe<'q> {
    ctx: &'q Ctx<'q>,
    quad: &'q QuadNamed,
    lowered: Vec<(SlotTerm, SlotTerm, SlotTerm)>,
    consts: Vec<(Option<u32>, Option<u32>)>,
    gslot: usize,
    sslot: usize,
    class: u32,
    scan: Pairs,
    subjects: Vec<u32>,
    cursor: usize,
    window: usize,
    out: std::collections::VecDeque<Row>,
}

impl ClassProbe<'_> {
    /// Probe the next window of subjects into `out`; `false` when there are no
    /// more (or a read failed, which the file records).
    fn fill(&mut self) -> bool {
        if self.cursor >= self.subjects.len() {
            return false;
        }
        let mut size = self.window;
        if let Some(demand) = self.ctx.limit_hint.get() {
            size = size.min(demand.max(1));
        }
        let end = (self.cursor + size).min(self.subjects.len());
        let subjects: Vec<u32> = self.subjects[self.cursor..end].to_vec();
        self.cursor = end;
        self.window = (self.window * 4).min(MAX_WINDOW);
        let Some(rows) = self.quad.subject_rows(&subjects, Some(self.class)) else {
            self.cursor = self.subjects.len();
            return false;
        };
        let dict = self.ctx.rete.dictionary();
        // The predicates of the window, canonicalized in one batch, and the
        // terms of the rows the projection will decode, with them.
        if dict.faults_chunks() {
            let mut preds: Vec<u32> = rows.iter().flatten().map(|r| r.1).collect();
            preds.sort_unstable();
            preds.dedup();
            let take = self.ctx.decode_rows();
            let mut nodes: Vec<u32> = Vec::new();
            if take > 0 {
                let decodes_s = self.ctx.decodes_slot(self.sslot);
                let o_slots: Vec<usize> = self
                    .lowered
                    .iter()
                    .filter_map(|t| match t.2 {
                        SlotTerm::Var(i) if self.ctx.decodes_slot(i) => Some(i),
                        _ => None,
                    })
                    .collect();
                for (s, rs) in subjects.iter().zip(&rows).take(take) {
                    if decodes_s {
                        nodes.push(dict.subject_node(*s));
                    }
                    if !o_slots.is_empty() {
                        nodes.extend(rs.iter().map(|r| dict.object_node(r.2)));
                    }
                }
                nodes.truncate(take.saturating_mul(4));
            }
            self.ctx.resolver.prefetch_canon(&preds, &nodes);
        }
        for (s, rs) in subjects.iter().zip(rows) {
            self.rows_of(*s, rs);
        }
        true
    }

    /// The solutions of the block for subject `s`, whose rows are `rs`: one set
    /// per graph where `s` has the class.
    fn rows_of(&mut self, s: u32, mut rs: Vec<Row4>) {
        let dict = self.ctx.rete.dictionary();
        // By graph, then by (p, o).
        rs.sort_unstable_by_key(|r| (r.3, r.1, r.2));
        let typed = self.scan.graphs_of(s);
        let mut base = self.ctx.slots.empty_row();
        base[self.sslot] = Some(Val::Id(dict.subject_node(s) as i64));
        for &(_, g) in typed {
            let lo = rs.partition_point(|r| r.3 < g);
            let hi = lo + rs[lo..].partition_point(|r| r.3 == g);
            if hi == lo {
                continue;
            }
            let mut row = base.clone();
            row[self.gslot] = Some(Val::Id(dict.subject_node(g) as i64));
            let mut out = std::mem::take(&mut self.out);
            self.star(&rs[lo..hi], 0, &row, &mut out);
            self.out = out;
        }
    }

    /// Extend `row` with every match of pattern `i` and the ones after it among
    /// `triples` (the subject's rows in one graph).
    fn star(
        &self,
        triples: &[Row4],
        i: usize,
        row: &Row,
        out: &mut std::collections::VecDeque<Row>,
    ) {
        if i == self.lowered.len() {
            crate::guard::check();
            out.push_back(row.clone());
            return;
        }
        let t = &self.lowered[i];
        let (cp, co) = self.consts[i];
        let dict = self.ctx.rete.dictionary();
        for r in triples {
            let (p, o) = (r.1, r.2);
            if cp.is_some_and(|x| x != p) || co.is_some_and(|x| x != o) {
                continue;
            }
            let mut next = row.clone();
            let mut ok = true;
            for (term, val) in [
                (&t.1, pred_val(self.ctx, &t.1, p)),
                (&t.2, dict.object_node(o) as i64),
            ] {
                if let SlotTerm::Var(k) = term {
                    match next[*k] {
                        Some(Val::Id(e)) if e != val => {
                            ok = false;
                            break;
                        }
                        Some(_) => {}
                        None => next[*k] = Some(Val::Id(val)),
                    }
                }
            }
            if ok {
                self.star(triples, i + 1, &next, out);
            }
        }
    }
}

impl Iterator for ClassProbe<'_> {
    type Item = Row;

    fn next(&mut self) -> Option<Row> {
        loop {
            if let Some(r) = self.out.pop_front() {
                return Some(r);
            }
            if !self.fill() {
                return None;
            }
        }
    }
}
